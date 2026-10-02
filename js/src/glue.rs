//! The global glue, evaluated once per realm before any page script.

pub const GLUE: &str = include_str!("glue.js");

/// Extract f32 payload from a typed-array JS value (Float32Array/Int32Array).
pub fn as_typed_f32(v: &rquickjs::Value) -> Option<Vec<f32>> {
    let arr = rquickjs::TypedArray::<f32>::from_value(v.clone()).ok()?;
    let raw = arr.as_raw()?;
    let f: &[f32] = unsafe { std::slice::from_raw_parts(raw.as_ptr().cast(), raw.len() / 4) };
    Some(f.to_vec())
}

/// Extract u8 payload from a typed-array JS value (Uint8Array).
pub fn as_typed_u8(v: &rquickjs::Value) -> Option<Vec<u8>> {
    let arr = rquickjs::TypedArray::<u8>::from_value(v.clone()).ok()?;
    let raw = arr.as_raw()?;
    let bytes: &[u8] = unsafe { std::slice::from_raw_parts(raw.as_ptr().cast(), raw.len()) };
    Some(bytes.to_vec())
}
