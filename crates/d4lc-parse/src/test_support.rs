use d4lc_assets::AssetRepository;
use d4lc_fuzzy::FuzzyIndexes;
use std::path::PathBuf;

pub(crate) fn load_enus_fuzzy_indexes() -> FuzzyIndexes {
    let repo = AssetRepository::new(workspace_root());
    FuzzyIndexes::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"))
}

pub(crate) fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    match manifest_dir.parent().and_then(|crates| crates.parent()) {
        Some(root) => root.to_path_buf(),
        None => panic!("could not resolve workspace root"),
    }
}
