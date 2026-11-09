//! `cargo xtask icons`: render the SVG sources in `assets/icon/` into the `.ico` files that the
//! Windows builds embed. The `.ico` files are committed, so building Burrow never needs this.

use std::fs;
use std::path::Path;

use resvg::{tiny_skia, usvg};

/// One `.ico` file, built from one or more SVG sources, each rendered at the listed pixel sizes.
struct Icon {
    output: &'static str,
    sources: &'static [(&'static str, &'static [u32])],
}

/// 100–300% display scaling of a 16 px tray icon.
const TRAY_SIZES: &[u32] = &[16, 20, 24, 32, 40, 48];

const ICONS: &[Icon] = &[
    // Explorer, the taskbar, Alt+Tab and the title bar. GPUI loads it as icon resource 1.
    Icon {
        output: "crates/burrow-app/resources/windows/burrow.ico",
        sources: &[
            ("assets/icon/burrow-small.svg", &[16, 20, 24]),
            ("assets/icon/burrow.svg", &[32, 40, 48, 64, 96, 128, 256]),
        ],
    },
    // Notification-area glyphs for burrowd, one per taskbar theme.
    Icon {
        output: "crates/burrowd/resources/windows/tray-dark-theme.ico",
        sources: &[("assets/icon/tray-dark-theme.svg", TRAY_SIZES)],
    },
    Icon {
        output: "crates/burrowd/resources/windows/tray-light-theme.ico",
        sources: &[("assets/icon/tray-light-theme.svg", TRAY_SIZES)],
    },
];

/// Writes every `.ico` file that differs from what its sources render to.
pub fn write(root: &Path) -> Result<(), String> {
    for icon in ICONS {
        let bytes = build(root, icon)?;
        let path = root.join(icon.output);
        if fs::read(&path).is_ok_and(|current| current == bytes) {
            println!("{} is up to date", icon.output);
            continue;
        }
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)
                .map_err(|error| format!("creating {}: {error}", dir.display()))?;
        }
        fs::write(&path, bytes).map_err(|error| format!("writing {}: {error}", path.display()))?;
        println!("wrote {}", icon.output);
    }
    Ok(())
}

/// Fails if any `.ico` file differs from what its sources render to.
pub fn check(root: &Path) -> Result<(), String> {
    let mut stale = Vec::new();
    for icon in ICONS {
        let bytes = build(root, icon)?;
        if fs::read(root.join(icon.output)).ok() != Some(bytes) {
            stale.push(icon.output);
        }
    }
    if stale.is_empty() {
        println!("icons are up to date");
        Ok(())
    } else {
        Err(format!(
            "out of date: {} (run `cargo xtask icons`)",
            stale.join(", ")
        ))
    }
}

fn build(root: &Path, icon: &Icon) -> Result<Vec<u8>, String> {
    let mut images = Vec::new();
    for &(source, sizes) in icon.sources {
        let path = root.join(source);
        let svg =
            fs::read(&path).map_err(|error| format!("reading {}: {error}", path.display()))?;
        let tree = usvg::Tree::from_data(&svg, &usvg::Options::default())
            .map_err(|error| format!("parsing {source}: {error}"))?;
        for &size in sizes {
            let png = render(&tree, size)
                .map_err(|error| format!("rendering {source} at {size} px: {error}"))?;
            images.push((size, png));
        }
    }
    images.sort_by_key(|&(size, _)| size);
    Ok(ico(&images))
}

/// Renders the SVG into a `size`×`size` PNG.
fn render(tree: &usvg::Tree, size: u32) -> Result<Vec<u8>, String> {
    let mut pixmap = tiny_skia::Pixmap::new(size, size).ok_or("icon size is zero")?;
    let scale_x = size as f32 / tree.size().width();
    let scale_y = size as f32 / tree.size().height();
    resvg::render(
        tree,
        tiny_skia::Transform::from_scale(scale_x, scale_y),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().map_err(|error| error.to_string())
}

/// Packs square PNG images into an ICO container. Windows reads PNG entries at every size.
fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    const HEADER: u32 = 6;
    const ENTRY: u32 = 16;
    let count = u16::try_from(images.len()).expect("fewer than 65536 images");
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&count.to_le_bytes());
    let mut offset = HEADER + ENTRY * u32::from(count);
    for (size, png) in images {
        // Width and height are one byte each; 0 means 256.
        let dimension = u8::try_from(*size).unwrap_or(0);
        let length = u32::try_from(png.len()).expect("PNG smaller than 4 GiB");
        out.extend_from_slice(&[dimension, dimension, 0, 0]); // size, no palette, reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += length;
    }
    for (_, png) in images {
        out.extend_from_slice(png);
    }
    out
}
