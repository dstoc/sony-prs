use wasm_bindgen::prelude::*;

/// Return a message from the Rust module after the browser loads it.
#[wasm_bindgen]
pub fn proof_of_life() -> String {
    "PRS-T1 reader web WASM is alive.".to_owned()
}
