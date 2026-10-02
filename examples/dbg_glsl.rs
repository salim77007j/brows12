fn main() {
    let vs100 = r#"
precision highp float;
attribute vec2 a_pos;
uniform mat4 u_mvp;
varying vec2 v_uv;
void main() {
  v_uv = a_pos;
  gl_Position = u_mvp * vec4(a_pos, 0.0, 1.0);
}
"#;
    let map = brows12_js::glsl::collect_bindings(vs100, "#version 300 es\nuniform sampler2D u_tex;\nuniform vec4 u_tint;\nin vec2 v_uv;\nout vec4 o_color;\nvoid main() { o_color = texture(u_tex, v_uv) * u_tint; }").unwrap();
    println!("MAP: {map:?}");
    println!("--- VS OUT ---");
    println!("{}", brows12_js::glsl::normalize(vs100, brows12_js::glsl::Stage::Vertex, &map).unwrap());
}
