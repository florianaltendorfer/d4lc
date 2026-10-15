use crate::{OcrError, OcrImage, OcrOptions, OcrProfile, OcrStructuredOutput};
use d4lc_core::OcrText;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub trait OcrEngine {
    fn recognize_lines(
        &mut self,
        image: &OcrImage,
        options: &OcrOptions,
    ) -> Result<OcrText, OcrError>;

    /// [`Self::recognize_lines`] plus the structured output the text was
    /// derived from.
    ///
    /// The default keeps the parsed text and reports no structured output.
    /// Backends that read structured output override this; when
    /// [`OcrOptions::collect_tsv`] is set and the structured output cannot be
    /// read, the call fails instead of returning a text-only result.
    fn recognize_structured(
        &mut self,
        image: &OcrImage,
        options: &OcrOptions,
    ) -> Result<OcrStructuredOutput, OcrError> {
        Ok(OcrStructuredOutput {
            text: self.recognize_lines(image, options)?,
            tsv: None,
        })
    }
}

pub type BoxedOcrEngine = Box<dyn OcrEngine + Send>;
type OcrEngineFactory = dyn Fn(&OcrProfile) -> Result<BoxedOcrEngine, OcrError> + Send + Sync;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrEnginePoolStats {
    pub initialized: u64,
    pub reused: u64,
    pub returned: u64,
    pub available: usize,
}

#[derive(Debug, Default)]
struct OcrEnginePoolCounters {
    initialized: AtomicU64,
    reused: AtomicU64,
    returned: AtomicU64,
}

/// Reusable OCR engine pool for one profile/language.
///
/// Upstream uses `ObjectPool<Engine>` and clears it when language changes. This pool keeps the
/// same shape in Rust: engines are mutable, reusable and never shared concurrently.
pub struct OcrEnginePool {
    profile: OcrProfile,
    engines: Mutex<Vec<BoxedOcrEngine>>,
    factory: Arc<OcrEngineFactory>,
    counters: OcrEnginePoolCounters,
}

impl OcrEnginePool {
    pub fn new(profile: OcrProfile, factory: Arc<OcrEngineFactory>) -> Self {
        Self {
            profile,
            engines: Mutex::new(Vec::new()),
            factory,
            counters: OcrEnginePoolCounters::default(),
        }
    }

    pub fn profile(&self) -> &OcrProfile {
        &self.profile
    }

    pub fn recognize_lines(
        &self,
        image: &OcrImage,
        options: &OcrOptions,
    ) -> Result<OcrText, OcrError> {
        let mut engine = self.take_engine()?;
        let result = engine.recognize_lines(image, options);
        self.return_engine(engine)?;
        result
    }

    pub fn recognize_structured(
        &self,
        image: &OcrImage,
        options: &OcrOptions,
    ) -> Result<OcrStructuredOutput, OcrError> {
        let mut engine = self.take_engine()?;
        let result = engine.recognize_structured(image, options);
        self.return_engine(engine)?;
        result
    }

    /// Initialize engines on `count` threads at once until the pool holds
    /// at least `count` idle engines, so the first parallel read does not
    /// pay for engine start-up. Fails with the first engine that cannot be
    /// created; engines created before stay in the pool.
    pub fn prewarm(&self, count: usize) -> Result<(), OcrError> {
        let missing = count.saturating_sub(self.available_engines()?);
        let created: Vec<Result<BoxedOcrEngine, OcrError>> = std::thread::scope(|scope| {
            let starts: Vec<_> = (0..missing)
                .map(|_| scope.spawn(|| (self.factory)(&self.profile)))
                .collect();
            starts
                .into_iter()
                .map(|start| match start.join() {
                    Ok(engine) => engine,
                    Err(panic) => std::panic::resume_unwind(panic),
                })
                .collect()
        });
        let mut first_error = None;
        for engine in created {
            match engine {
                Ok(engine) => {
                    self.counters.initialized.fetch_add(1, Ordering::SeqCst);
                    self.engines
                        .lock()
                        .map_err(|_| OcrError::PoolPoisoned)?
                        .push(engine);
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    pub fn available_engines(&self) -> Result<usize, OcrError> {
        self.engines
            .lock()
            .map(|engines| engines.len())
            .map_err(|_| OcrError::PoolPoisoned)
    }

    pub fn stats(&self) -> Result<OcrEnginePoolStats, OcrError> {
        Ok(OcrEnginePoolStats {
            initialized: self.counters.initialized.load(Ordering::SeqCst),
            reused: self.counters.reused.load(Ordering::SeqCst),
            returned: self.counters.returned.load(Ordering::SeqCst),
            available: self.available_engines()?,
        })
    }

    pub fn clear(&self) -> Result<(), OcrError> {
        self.engines
            .lock()
            .map(|mut engines| engines.clear())
            .map_err(|_| OcrError::PoolPoisoned)
    }

    fn take_engine(&self) -> Result<BoxedOcrEngine, OcrError> {
        let maybe_engine = self
            .engines
            .lock()
            .map_err(|_| OcrError::PoolPoisoned)?
            .pop();
        match maybe_engine {
            Some(engine) => {
                self.counters.reused.fetch_add(1, Ordering::SeqCst);
                Ok(engine)
            }
            None => {
                self.counters.initialized.fetch_add(1, Ordering::SeqCst);
                (self.factory)(&self.profile)
            }
        }
    }

    fn return_engine(&self, engine: BoxedOcrEngine) -> Result<(), OcrError> {
        self.counters.returned.fetch_add(1, Ordering::SeqCst);
        self.engines
            .lock()
            .map(|mut engines| engines.push(engine))
            .map_err(|_| OcrError::PoolPoisoned)
    }
}

/// Registry that creates one pool per OCR profile/language.
pub struct OcrEngineRegistry {
    pools: Mutex<HashMap<OcrProfile, Arc<OcrEnginePool>>>,
    factory: Arc<OcrEngineFactory>,
}

impl OcrEngineRegistry {
    pub fn new(
        factory: impl Fn(&OcrProfile) -> Result<BoxedOcrEngine, OcrError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            pools: Mutex::new(HashMap::new()),
            factory: Arc::new(factory),
        }
    }

    pub fn pool_for(&self, profile: OcrProfile) -> Result<Arc<OcrEnginePool>, OcrError> {
        let mut pools = self.pools.lock().map_err(|_| OcrError::PoolPoisoned)?;
        if let Some(pool) = pools.get(&profile) {
            return Ok(Arc::clone(pool));
        }

        let pool = Arc::new(OcrEnginePool::new(
            profile.clone(),
            Arc::clone(&self.factory),
        ));
        pools.insert(profile, Arc::clone(&pool));
        Ok(pool)
    }

    pub fn recognize_lines(
        &self,
        profile: OcrProfile,
        image: &OcrImage,
        options: &OcrOptions,
    ) -> Result<OcrText, OcrError> {
        self.pool_for(profile)?.recognize_lines(image, options)
    }

    pub fn recognize_structured(
        &self,
        profile: OcrProfile,
        image: &OcrImage,
        options: &OcrOptions,
    ) -> Result<OcrStructuredOutput, OcrError> {
        self.pool_for(profile)?.recognize_structured(image, options)
    }

    pub fn clear_profile(&self, profile: &OcrProfile) -> Result<(), OcrError> {
        if let Some(pool) = self
            .pools
            .lock()
            .map_err(|_| OcrError::PoolPoisoned)?
            .get(profile)
        {
            pool.clear()?;
        }
        Ok(())
    }

    pub fn switch_language(
        &self,
        previous_profile: &OcrProfile,
        language: &str,
    ) -> Result<OcrProfile, OcrError> {
        self.clear_profile(previous_profile)?;
        let mut next_profile = previous_profile.clone();
        next_profile.language = language.to_string();
        Ok(next_profile)
    }
}

pub struct UnavailableOcrEngine {
    reason: String,
}

impl UnavailableOcrEngine {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl OcrEngine for UnavailableOcrEngine {
    fn recognize_lines(
        &mut self,
        _image: &OcrImage,
        _options: &OcrOptions,
    ) -> Result<OcrText, OcrError> {
        Err(OcrError::BackendUnavailable(self.reason.clone()))
    }
}

#[cfg(test)]
mod tests;
