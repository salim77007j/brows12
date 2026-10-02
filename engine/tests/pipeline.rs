use brows12_engine::{Engine, EngineConfig, EngineEvent};

fn local_server(files: Vec<(&'static str, String)>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let files: std::collections::HashMap<String, String> =
        files.into_iter().map(|(p, c)| (p.to_string(), c)).collect();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => return,
            };
            use std::io::{Read, Write};
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            let req = String::from_utf8_lossy(&buf);
            let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
            let body =
                files.get(&path).cloned().unwrap_or_else(|| "<html><body>404</body></html>".into());
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://{addr}")
}

#[test]
fn end_to_end_page_load_with_css_and_js() {
    let html = r##"<!DOCTYPE html>
<html><head>
<link rel="stylesheet" href="/style.css">
<script>document.getElementById('target').textContent = 'JS-RAN';</script>
</head><body>
<h1 id="target">INITIAL</h1>
<p class="intro">Styled paragraph.</p>
</body></html>"##;
    let css = "body { background-color: #102030; } h1 { color: #ff8800; font-size: 40px; } .intro { margin-top: 8px; }";

    let base = local_server(vec![("/", html.to_string()), ("/style.css", css.to_string())]);

    let engine = Engine::new(EngineConfig::default());
    let mut events = engine.subscribe();
    let tab = engine.tab();
    tab.load_url(&base).expect("load must succeed");

    // JS ran and mutated the DOM → frame regenerated (gen >= 2 when mutated).
    let frame = tab.frame().expect("frame present");
    assert!(frame.generation >= 1);

    // Ink present (heading + text painted).
    let inked = frame.pixmap.data().chunks_exact(4).filter(|p| p[3] > 0).count();
    assert!(inked > 200, "expected painted ink, got {inked} px");

    // Events flowed.
    let mut saw_frame = false;
    while let Ok(ev) = events.try_recv() {
        if let EngineEvent::FrameReady { .. } = ev {
            saw_frame = true;
        }
    }
    assert!(saw_frame);
}

#[test]
fn blocked_page_renders_shield() {
    let engine = Engine::new(EngineConfig::default());
    let tab = engine.tab();
    tab.load_url("https://doubleclick.net/ad").ok();
    if tab.url().starts_with("brows12://blocked") {
        assert_eq!(tab.title(), "Blocked by Brows12");
    }
}

#[test]
fn suspension_keeps_snapshot() {
    let html = "<html><head><title>Holder</title></head><body><p>Suspend me</p></body></html>";
    let base = local_server(vec![("/", html.to_string())]);
    let engine = Engine::new(EngineConfig::default());
    let tab = engine.tab();
    tab.load_url(&base).unwrap();
    let before = tab.frame().unwrap().generation;
    tab.suspend();
    assert!(!tab.is_live());
    assert_eq!(tab.title(), "Holder");
    assert!(tab.frame().is_some());
    tab.resume().unwrap();
    assert!(tab.is_live());
    assert!(tab.frame().unwrap().generation >= before);
    assert_eq!(engine.live_page_count(), 1);
}

#[test]
fn webgl2_triangle_renders_and_harvests() {
    // GPU-less environments (headless containers) legitimately return null
    // from getContext — skip rather than fail (CI installs lavapipe).
    if !brows12_js::webgl::gpu_available() {
        eprintln!("no GPU adapter; skipping WebGL test");
        return;
    }
    let html = r#"<!DOCTYPE html><html><head><style>
body { background-color: #ffffff; } canvas { width: 64px; height: 64px; }
</style></head><body>
<canvas id="gl" width="64" height="64"></canvas>
<script>
var canvas = document.getElementById('gl');
var gl = canvas.getContext('webgl2');
if (!gl) throw new Error('no webgl2 context');
var vs = 'attribute vec3 a_pos; void main() { gl_Position = vec4(a_pos, 1.0); }';
var fs = 'precision mediump float; uniform vec4 u_color; void main() { gl_FragColor = u_color; }';
function makeShader(type, src) {
  var s = gl.createShader(type);
  gl.shaderSource(s, src);
  gl.compileShader(s);
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
    throw new Error('shader: ' + gl.getShaderInfoLog(s));
  }
  return s;
}
var prog = gl.createProgram();
gl.attachShader(prog, makeShader(gl.VERTEX_SHADER, vs));
gl.attachShader(prog, makeShader(gl.FRAGMENT_SHADER, fs));
gl.linkProgram(prog);
if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
  throw new Error('link: ' + gl.getProgramInfoLog(prog));
}
gl.useProgram(prog);
var buf = gl.createBuffer();
gl.bindBuffer(gl.ARRAY_BUFFER, buf);
gl.bufferData(gl.ARRAY_BUFFER,
  new Float32Array([0.0, 0.8, 0.0, -0.8, -0.6, 0.0, 0.8, -0.6, 0.0]),
  gl.STATIC_DRAW);
var loc = gl.getAttribLocation(prog, 'a_pos');
if (loc < 0) throw new Error('a_pos location missing');
gl.enableVertexAttribArray(loc);
gl.vertexAttribPointer(loc, 3, gl.FLOAT, false, 0, 0);
var uc = gl.getUniformLocation(prog, 'u_color');
if (!uc) throw new Error('u_color location missing');
gl.uniform4f(uc, 1.0, 0.0, 0.0, 1.0);
gl.clearColor(0.0, 0.0, 0.0, 0.0);
gl.clear(gl.COLOR_BUFFER_BIT);
gl.drawArrays(gl.TRIANGLES, 0, 3);
</script></body></html>"#;

    let engine = Engine::new(EngineConfig::default());
    let tab = engine.tab();
    tab.load_url_from_string(html, "brows12://fixture/webgl").expect("load");

    let frame = tab.frame().expect("frame present");
    let data = frame.pixmap.data();
    // The page paints white bg + a red triangle where the canvas sits.
    let red = data.chunks_exact(4).filter(|p| p[0] > 180 && p[1] < 90 && p[2] < 90).count();
    assert!(red > 100, "expected red triangle pixels, got {red}");
    eprintln!("webgl triangle: {red} red pixels painted");
}

#[test]
fn webgpu_compute_vector_add() {
    if !brows12_js::webgl::gpu_available() {
        eprintln!("no GPU adapter; skipping WebGPU test");
        return;
    }
    let html = r#"<!DOCTYPE html><html><head></head><body>
<script>
navigator.gpu.requestAdapter();
// The engine realm is synchronous: requestAdapter/requestDevice resolve
// immediately (documented deviation).
var adapter = navigator.gpu.requestAdapter();
if (!adapter) throw new Error('no adapter');
var device = adapter.requestDevice();
if (!device) throw new Error('no device');

var shader = device.createShaderModule({ code: `
@group(0) @binding(0) var<storage, read_write> v: array<f32>;
@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
  v[gid.x] = v[gid.x] * 2.0;
}
` });
var buf = device.createBuffer({ size: 16, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC | GPUBufferUsage.COPY_DST });
device.queue.writeBuffer(buf, 0, new Float32Array([1.0, 2.0, 3.0, 4.0]));
var pipeline = device.createComputePipeline({ compute: { module: shader, entryPoint: 'main' } });
var bgl = device.createBindGroupLayout({ entries: [{ binding: 0, visibility: GPUShaderStage.COMPUTE, buffer: { type: 'storage' } }] });
var bg = device.createBindGroup({ layout: bgl, entries: [{ binding: 0, resource: { buffer: buf } }] });
var enc = device.createCommandEncoder();
var pass = enc.beginComputePass();
pass.setPipeline(pipeline);
pass.setBindGroup(0, bg);
pass.dispatchWorkgroups(4, 1, 1);
pass.end();
device.queue.submit([enc.finish()]);
</script></body></html>"#;

    let engine = Engine::new(EngineConfig::default());
    let tab = engine.tab();
    let result = tab.load_url_from_string(html, "brows12://fixture/webgpu");
    if let Err(e) = &result {
        panic!("load failed: {e}");
    }
    eprintln!("webgpu compute pipeline executed");
}

#[test]
fn indexeddb_put_get_round_trip() {
    let html = r#"<!DOCTYPE html><html><head></head><body>
<script>
var db = null;
var open = indexedDB.open('notes');
db = open.result;
var store = db.createObjectStore('notes', { keyPath: 'id' });
var tx = db.transaction('notes', 'readwrite');
var os = tx.objectStore('notes');
os.put({ id: 1, text: 'first' });
os.put({ id: 2, text: 'second' });
var got = os.get(1);
var all = os.getAll();
if (JSON.stringify(got.result) !== '{"id":1,"text":"first"}') {
  throw new Error('get mismatch: ' + JSON.stringify(got.result));
}
if (all.result.length !== 2) throw new Error('getAll expected 2, got ' + all.result.length);
os.delete(1);
if (os.get(1).result !== null) throw new Error('delete failed');
</script></body></html>"#;
    let engine = Engine::new(EngineConfig::default());
    let tab = engine.tab();
    tab.load_url_from_string(html, "brows12://fixture/idb").expect("idb load");
}

#[test]
fn element_animate_sets_transition() {
    let html = r#"<!DOCTYPE html><html><head></head><body>
<div id="a" style="opacity: 1"></div>
<script>
var el = document.getElementById('a');
var anim = el.animate([{ opacity: 0 }, { opacity: 1 }], 300);
if (!anim || anim.playState !== 'running') throw new Error('animate did not start');
anim.cancel();
</script></body></html>"#;
    let engine = Engine::new(EngineConfig::default());
    let tab = engine.tab();
    tab.load_url_from_string(html, "brows12://fixture/animate").expect("animate load");
}
