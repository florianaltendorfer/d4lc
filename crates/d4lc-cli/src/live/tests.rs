use super::*;

#[test]
fn live_loop_default_frame_bound_is_100() {
    let args = LiveArgs::default();
    assert_eq!(args.frames.unwrap_or(100), 100);
    assert_eq!(args.interval_millis.unwrap_or(0), 0);
    assert!(!args.overlay);
}

#[test]
fn default_live_args_disable_debug_output() {
    let args = LiveArgs::default();
    assert!(!args.debug_enabled());
    assert_eq!(args.output_dir(), None);
}

#[test]
fn live_args_default_does_not_enable_summary_only() {
    let args = LiveArgs::default();

    assert!(!args.summary_only);
}
