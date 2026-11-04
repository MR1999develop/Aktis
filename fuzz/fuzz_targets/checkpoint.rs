//! The PyTorch checkpoint (`.pth`: a zip with a pickle program) that the
//! AI denoise model is made from.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = scunet_onnx::load_state_dict(data);
});
