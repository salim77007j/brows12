fn main() {
    let vs = "precision highp float;\nattribute vec3 a_pos;\nattribute vec2 a_uv;\nuniform mat4 u_mvp;\nvarying vec2 v_uv;\nvoid main() { v_uv = a_uv; gl_Position = u_mvp * vec4(a_pos, 1.0); }";
    let fs = "#version 300 es\nprecision mediump float;\nuniform sampler2D u_tex;\nuniform vec4 u_tint;\nin vec2 v_uv;\nout vec4 o_color;\nvoid main() { o_color = texture(u_tex, v_uv) * u_tint; }";
    let map = brows12_js::glsl::collect_bindings(vs, fs).unwrap();
    println!("MAP: {map:?}");
    let nv = brows12_js::glsl::normalize(vs, brows12_js::glsl::Stage::Vertex, &map).unwrap();
    let nf = brows12_js::glsl::normalize(fs, brows12_js::glsl::Stage::Fragment, &map).unwrap();
    println!("=== NV ===\n{nv}\n=== NF ===\n{nf}");
}
