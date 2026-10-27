//! The real network, when its model is available: run with
//! `RNE_DENOISE_MODELS=<folder> cargo test -p tool-denoise-ai --release -- --ignored --nocapture`.
//! CI generates the 448 px model from the original weights and runs this.

use std::path::PathBuf;
use std::time::Instant;

use core_model::{ImageF32, ParamId, ParamValue, Params, WorkingSpace};
use tool_api::{ResolvedParams, Tool};
use tool_denoise_ai::engine::{DEVICE_ENV, Device};
use tool_denoise_ai::models::MODELS_ENV;
use tool_denoise_ai::{AiDenoise, COLOR, GRAIN};

/// Gradients and squares in linear light, plus reproducible noise added in
/// the encoded domain (where the network judges it), clean and noisy.
fn scene(w: u32, h: u32) -> (ImageF32, ImageF32) {
    let mut state = 7_u32;
    let mut noise = || {
        // sum of uniforms ≈ Gaussian, standard deviation ≈ 0.06 (encoded)
        let mut s = 0.0_f32;
        for _ in 0..4 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            #[allow(clippy::cast_precision_loss)]
            let u = (state >> 8) as f32 / (1_u32 << 24) as f32;
            s += u - 0.5;
        }
        s * 0.104
    };
    let mut clean = Vec::new();
    let mut noisy = Vec::new();
    for y in 0..h {
        for x in 0..w {
            #[allow(clippy::cast_precision_loss)]
            let base = 0.2 + 0.6 * x as f32 / w as f32;
            let square = if (x / 40 + y / 40).is_multiple_of(2) { 0.1 } else { -0.1 };
            let mut c = [0.0; 4];
            let mut n = [0.0; 4];
            for k in 0..3_u8 {
                let v: f32 = (base + square + 0.05 * f32::from(k)).clamp(0.0, 1.0);
                c[usize::from(k)] = core_color::srgb_decode(v);
                n[usize::from(k)] = core_color::srgb_decode((v + noise()).clamp(0.0, 1.0));
            }
            c[3] = 1.0;
            n[3] = 1.0;
            clean.push(c);
            noisy.push(n);
        }
    }
    let img = |p| ImageF32::from_pixels(w, h, WorkingSpace::LinearRec2020, p).expect("valid size");
    (img(clean), img(noisy))
}

/// RMSE in the encoded domain, where noise is judged.
fn rmse(a: &ImageF32, b: &ImageF32) -> f32 {
    let mut sum = 0.0_f64;
    for (p, q) in a.pixels().iter().zip(b.pixels()) {
        for (u, v) in p.iter().zip(q).take(3) {
            let d = f64::from(core_color::srgb_encode(*u) - core_color::srgb_encode(*v));
            sum += d * d;
        }
    }
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let r = (sum / (a.pixels().len() * 3) as f64).sqrt() as f32;
    r
}

#[test]
#[ignore = "needs the SCUNet model: set RNE_DENOISE_MODELS"]
fn the_network_removes_most_of_the_noise() {
    let Some(dir) = std::env::var_os(MODELS_ENV) else { panic!("set {MODELS_ENV} to the model folder") };
    let device = std::env::var(DEVICE_ENV).map_or(Device::Auto, |v| Device::parse(&v));
    let tool = AiDenoise::with_model_dirs(vec![PathBuf::from(dir)], device);

    let mut p = Params::new();
    p.set(ParamId::new(GRAIN).expect("id"), ParamValue::Float(100.0));
    p.set(ParamId::new(COLOR).expect("id"), ParamValue::Float(100.0));
    let params = ResolvedParams::resolve(tool.descriptor(), &p);

    // 600×500 covers 2×2 useful tiles of 320 px: the seams are checked too.
    let (clean, noisy) = scene(600, 500);
    let mut out = noisy.clone();
    let t = Instant::now();
    tool.apply(&mut out, &params).expect("the network runs");
    let first = t.elapsed().as_secs_f32();

    // Moving the sliders reuses the network output: must be instant.
    let mut again = noisy.clone();
    let t = Instant::now();
    tool.apply(&mut again, &params).expect("runs");
    let second = t.elapsed().as_secs_f32();

    let (before, after) = (rmse(&noisy, &clean), rmse(&out, &clean));
    println!("engine: {}", tool.status().unwrap_or_default());
    println!("RMSE {before:.4} -> {after:.4}; first run {first:.1} s, cached {second:.3} s");
    assert!(after < before * 0.5, "noise not reduced enough: {before} -> {after}");
    assert_eq!(out, again, "the cached output gives the same result");
    assert!(second < first / 4.0, "the second run did not use the cache");
}
