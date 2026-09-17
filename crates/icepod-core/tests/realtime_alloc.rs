use assert_no_alloc::{AllocDisabler, assert_no_alloc, reset_violation_count, violation_count};
use icepod_core::CaptureCallback;

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

#[test]
fn initialized_callback_does_not_allocate_or_free() {
    let (mut callback, _worker) = CaptureCallback::new(8, 256, 2);
    let samples = [0.25_f32; 128];
    reset_violation_count();
    assert_no_alloc(|| callback.capture(&samples, Some(42)));
    assert_eq!(violation_count(), 0);
}

#[test]
fn callback_stays_below_a_buffer_period_on_this_host() {
    let (mut callback, mut worker) = CaptureCallback::new(8, 256, 2);
    let samples = [0.25_f32; 512];
    let mut maximum = std::time::Duration::ZERO;
    for sequence in 0..1000 {
        let started = std::time::Instant::now();
        assert_no_alloc(|| callback.capture(&samples, Some(sequence)));
        maximum = maximum.max(started.elapsed());
        while let Some(block) = worker.pop() {
            worker.recycle(block).unwrap();
        }
    }
    assert!(
        maximum < std::time::Duration::from_micros(5_333),
        "maximum callback time: {maximum:?}"
    );
}
