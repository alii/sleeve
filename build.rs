//! Gives the Windows exe its icon, made from the same `assets/AppIcon.icon` the macOS app's
//! icon is compiled from, so there's one icon to keep up to date.
//!
//! macOS has `actool` to compile an Icon Composer icon. Windows has nothing like it, so this
//! reads `icon.json` and draws it: the background, then each layer's SVG at its opacity, at the
//! sizes Windows asks for. The glass, blur and shadow effects are Apple's own and aren't drawn.
//! The sizes go into an .ico, and that into the exe as a resource with the name and version.

use std::path::{Path, PathBuf};
use std::{env, fs};

use resvg::tiny_skia::{Pixmap, PixmapPaint, Transform};
use resvg::usvg;
use serde_json::Value;

const ICON: &str = "assets/AppIcon.icon";
/// The size the layers are drawn at in Icon Composer.
const CANVAS: f32 = 1024.0;
/// The sizes Explorer, the taskbar and Alt+Tab ask for, at the usual display scales.
const SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={ICON}");
    // This runs on the machine doing the build, so `cfg!(windows)` would say what that is.
    // What matters is what the exe is for.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let ico = out.join("sleeve.ico");
    fs::write(&ico, ico_file(&SIZES.map(|n| draw(Path::new(ICON), n).encode_png().unwrap()))).unwrap();
    let rc = out.join("sleeve.rc");
    fs::write(&rc, resource_script(&ico)).unwrap();
    embed_resource::compile(&rc, embed_resource::NONE).manifest_optional().unwrap();
}

/// The icon at `size` pixels square.
fn draw(icon: &Path, size: u32) -> Pixmap {
    let json: Value = serde_json::from_slice(&fs::read(icon.join("icon.json")).expect("reading icon.json")).expect("icon.json isn't valid JSON");
    let mut canvas = Pixmap::new(size, size).unwrap();
    let scale = size as f32 / CANVAS;
    canvas.draw_pixmap(0, 0, render(&background(&json), scale, size).as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    // Icon Composer lists groups, and the layers in them, front first: draw from the back.
    let groups = json["groups"].as_array().map(Vec::as_slice).unwrap_or_default();
    for group in groups.iter().rev() {
        let layers = group["layers"].as_array().map(Vec::as_slice).unwrap_or_default();
        for layer in layers.iter().rev() {
            if layer["hidden"].as_bool() == Some(true) {
                continue;
            }
            let name = layer["image-name"].as_str().expect("a layer without an image-name");
            let svg = fs::read_to_string(icon.join("Assets").join(name)).unwrap_or_else(|e| panic!("reading {name}: {e}"));
            let opacity = layer["opacity"].as_f64().unwrap_or(1.0) as f32;
            let paint = PixmapPaint { opacity, ..PixmapPaint::default() };
            canvas.draw_pixmap(0, 0, render(&svg, scale, size).as_ref(), &paint, Transform::identity(), None);
        }
    }
    canvas
}

fn render(svg: &str, scale: f32, size: u32) -> Pixmap {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).expect("an icon SVG that doesn't parse");
    let mut pixmap = Pixmap::new(size, size).unwrap();
    resvg::render(&tree, Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap
}

/// The tile behind the layers, in the icon's fill (its light one; Windows uses the same icon in
/// light and dark mode). macOS masks the icon to its own rounded square; this is a close one,
/// a little in from the edges, as Windows icons are.
fn background(json: &Value) -> String {
    let fills = json["fill-specializations"].as_array().map(Vec::as_slice).unwrap_or_default();
    let fill = fills.iter().find(|f| f.get("appearance").is_none()).map(|f| &f["value"]).or_else(|| json.get("fill")).expect("icon.json has no fill");
    let (top, bottom) = if let Some(stops) = fill["linear-gradient"].as_array() {
        (colour(&stops[0]), colour(&stops[stops.len() - 1]))
    } else if let Some(solid) = fill.get("solid") {
        (colour(solid), colour(solid))
    } else {
        panic!("icon.json's fill is neither a linear gradient nor solid: {fill}");
    };
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">
  <linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{top}"/><stop offset="1" stop-color="{bottom}"/></linearGradient>
  <rect x="32" y="32" width="960" height="960" rx="216" fill="url(#g)"/>
</svg>"#
    )
}

/// An Icon Composer colour, such as `display-p3:0.25,0.50,1.00,1.00000`, as an sRGB hex colour.
fn colour(v: &Value) -> String {
    let s = v.as_str().expect("a colour that isn't a string");
    let (space, nums) = s.split_once(':').unwrap_or(("srgb", s));
    let c: Vec<f32> = nums.split(',').map(|n| n.trim().parse().expect("a colour component that isn't a number")).collect();
    let mut rgb = [c[0], c[1], c[2]];
    if space == "display-p3" {
        // Display P3 has the same curve as sRGB but wider primaries: undo the curve, change the
        // primaries, then put the curve back. A colour outside sRGB is clipped to its edge.
        let lin = rgb.map(|x| if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) });
        let m = [[1.224_940_2, -0.224_940_2, 0.0], [-0.042_056_955, 1.042_057, 0.0], [-0.019_637_555, -0.078_636_05, 1.098_273_6]];
        rgb = m.map(|row| row[0] * lin[0] + row[1] * lin[1] + row[2] * lin[2]).map(|x| {
            let x = x.clamp(0.0, 1.0);
            if x <= 0.003_130_8 { x * 12.92 } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 }
        });
    }
    let [r, g, b] = rgb.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// An .ico holding each size as a PNG, which Windows has read since Vista: a 6-byte header, a
/// 16-byte entry for each image saying where its bytes are, then the images.
fn ico_file(pngs: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(0u16.to_le_bytes()); // reserved
    out.extend(1u16.to_le_bytes()); // 1 = icon (2 would be a cursor)
    out.extend((pngs.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * pngs.len() as u32;
    for (png, size) in pngs.iter().zip(SIZES) {
        let side = (size % 256) as u8; // 0 means 256
        out.extend([side, side, 0, 0]); // width, height, no palette, reserved
        out.extend(1u16.to_le_bytes()); // colour planes
        out.extend(32u16.to_le_bytes()); // bits per pixel
        out.extend((png.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for png in pngs {
        out.extend(png);
    }
    out
}

/// The resource script: the icon, and the name and version Explorer's Properties and Task
/// Manager show.
fn resource_script(ico: &Path) -> String {
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let numbers = ["MAJOR", "MINOR", "PATCH"].map(|p| env::var(format!("CARGO_PKG_VERSION_{p}")).unwrap()).join(",");
    // Backslashes in a resource script's strings are escapes, as in C.
    let ico = ico.display().to_string().replace('\\', "\\\\");
    format!(
        r#"1 ICON "{ico}"
1 VERSIONINFO
FILEVERSION {numbers},0
PRODUCTVERSION {numbers},0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "ProductName", "Sleeve"
      VALUE "FileDescription", "Sleeve"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
      VALUE "OriginalFilename", "sleeve.exe"
      VALUE "LegalCopyright", "GPL-2.0-or-later"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    )
}
