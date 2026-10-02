fn main() {
    let map = brows12_js::glsl::collect_bindings(
        "attribute vec2 a_pos;\nuniform mat4 u_mvp;\nvarying vec2 v_uv;\nvoid main() { gl_Position = u_mvp * vec4(a_pos, 0.0, 1.0); }",
        "#version 300 es\nprecision mediump float;\nuniform sampler2D u_tex;\nuniform vec4 u_tint;\nin vec2 v_uv;\nout vec4 o_color;\nvoid main() { o_color = texture(u_tex, v_uv) * u_tint; }",
    ).unwrap();
    println!("MAP: {map:?}");
    let fs = "#version 300 es\nprecision mediump float;\nuniform sampler2D u_tex;\nuniform vec4 u_tint;\nin vec2 v_uv;\nout vec4 o_color;\nvoid main() { o_color = texture(u_tex, v_uv) * u_tint; }";
    println!("{}", brows12_js::glsl::normalize(fs, brows12_js::glsl::Stage::Fragment, &map).unwrap());
}
