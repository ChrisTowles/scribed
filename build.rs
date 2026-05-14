// No build-script work needed: with sherpa-onnx's `static` feature the
// native library is statically linked into the binary, so we don't need an
// `$ORIGIN` rpath to find a sibling `.so` at runtime.

fn main() {}
