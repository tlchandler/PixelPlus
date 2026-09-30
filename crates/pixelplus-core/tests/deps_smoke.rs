//! rustfft (added for F2 audio analysis by WS0) is usable from core.

#[test]
fn rustfft_forward_transform() {
    let mut planner = rustfft::FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(8);
    let mut buf = vec![rustfft::num_complex::Complex::new(1.0f32, 0.0); 8];
    fft.process(&mut buf);
    assert!((buf[0].re - 8.0).abs() < 1e-5);
    assert!(buf[1..].iter().all(|c| c.norm() < 1e-5));
}
