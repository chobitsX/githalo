use std::path::Path;

use githalo::theme as app_theme;
use gpui::{IntoElement, prelude::*, rems, rgb, svg};

use super::components::theme;

#[derive(Clone, Copy)]
struct IconColors {
    rust: u32,
    ts: u32,
    js: u32,
    py: u32,
    go: u32,
    java: u32,
    kotlin: u32,
    c: u32,
    swift: u32,
    csharp: u32,
    shell: u32,
    sql: u32,
    html: u32,
    css: u32,
    vue: u32,
    md: u32,
    json: u32,
    yaml: u32,
    config: u32,
    csv: u32,
    pdf: u32,
    doc: u32,
    license: u32,
    git: u32,
    docker: u32,
    lock: u32,
    image: u32,
    archive: u32,
    binary: u32,
    folder: u32,
    folder_open: u32,
    folder_special: u32,
}

// Brand-harmonized dark theme file icon palette
const DARK_ICONS: IconColors = IconColors {
    rust: 0xe38b59,
    ts: 0x4ea1f3,
    js: 0xf1d465,
    py: 0x5ba8e8,
    go: 0x5ccfe6,
    java: 0xe69457,
    kotlin: 0xb388ff,
    c: 0x6f9ef5,
    swift: 0xf27c54,
    csharp: 0x7bc98d,
    shell: 0x86c98e,
    sql: 0x56c7b8,
    html: 0xe57356,
    css: 0x66a3ff,
    vue: 0x5dc983,
    md: 0x6bb4ff,
    json: 0xe5b473,
    yaml: 0xd97bb8,
    config: 0xb794f4,
    csv: 0x6bc48e,
    pdf: 0xeb7878,
    doc: 0x76a8f4,
    license: 0xdcb86a,
    git: 0xf06543,
    docker: 0x4aa8eb,
    lock: 0xc4a265,
    image: 0xd089e8,
    archive: 0xd4a056,
    binary: 0x8e96a6,
    folder: 0x8a97ab,
    folder_open: 0x9bb0cf,
    folder_special: 0x6f9ad8,
};

// High-contrast crisp light theme file icon palette
const LIGHT_ICONS: IconColors = IconColors {
    rust: 0xc25524,
    ts: 0x1e6fc4,
    js: 0xb88408,
    py: 0x246eb3,
    go: 0x0b88a3,
    java: 0xc45e1a,
    kotlin: 0x7444cf,
    c: 0x2f63c7,
    swift: 0xd14c24,
    csharp: 0x268241,
    shell: 0x268247,
    sql: 0x0d8879,
    html: 0xc9492a,
    css: 0x2466cf,
    vue: 0x1a874f,
    md: 0x2068c2,
    json: 0xb57514,
    yaml: 0xb03a86,
    config: 0x764bc7,
    csv: 0x1f8249,
    pdf: 0xc9363d,
    doc: 0x2968c4,
    license: 0xa67814,
    git: 0xcf4120,
    docker: 0x1678ba,
    lock: 0x996d1e,
    image: 0x9a42bd,
    archive: 0xa86b18,
    binary: 0x5c6678,
    folder: 0x64748b,
    folder_open: 0x475569,
    folder_special: 0x2e64b0,
};

#[inline]
fn icon_colors() -> &'static IconColors {
    if app_theme::is_light() {
        &LIGHT_ICONS
    } else {
        &DARK_ICONS
    }
}

// Crisp 16x16 monochrome SVG paths rendered via GPUI's native resvg/usvg pipeline + brand tint.
const SVG_FILE_DEFAULT: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M4 2h5.2L12.5 5.3V14H4z"/><path d="M9.2 2v3.3h3.3"/></svg>"##;
const SVG_FOLDER_CLOSED: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M1.8 3.8h4.1l1.5 1.6h6.8v7.4H1.8z"/></svg>"##;
const SVG_FOLDER_OPEN: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M1.8 3.8h4l1.4 1.5h6.6v1.9H3.4L1.8 12.6z"/><path d="M3.4 7.2h11.1l-1.5 5.4H1.8z"/></svg>"##;
const SVG_RUST: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.35" stroke-linecap="round" stroke-linejoin="round"><circle cx="8" cy="8" r="4.3"/><path d="M8 1.9v1.8M8 12.3v1.8M1.9 8h1.8M12.3 8h1.8M3.7 3.7l1.2 1.2M11.1 11.1l1.2 1.2M12.3 3.7l-1.2 1.2M4.9 11.1l-1.2 1.2"/><circle cx="8" cy="8" r="1.4" fill="currentColor" stroke="none"/></svg>"##;
const SVG_MARKDOWN: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round"><rect x="1.6" y="3.2" width="12.8" height="9.6" rx="1.6"/><path d="M3.7 10.3V5.7l1.8 2.2 1.8-2.2v4.6"/><path d="M11.1 5.7v4.4M9.4 8.6l1.7 1.7 1.7-1.7"/></svg>"##;
const SVG_CODE_BRACKETS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.35" stroke-linecap="round" stroke-linejoin="round"><path d="M5.4 4.3L2 8l3.4 3.7"/><path d="M10.6 4.3L14 8l-3.4 3.7"/><path d="M9.1 3.3L6.9 12.7"/></svg>"##;
const SVG_BRACES: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.35" stroke-linecap="round" stroke-linejoin="round"><path d="M5.6 2.8C4.4 2.8 4 3.4 4 4.4v2c0 .9-.4 1.6-1.3 1.6.9 0 1.3.7 1.3 1.6v2c0 1 .4 1.6 1.6 1.6"/><path d="M10.4 2.8c1.2 0 1.6.6 1.6 1.6v2c0 .9.4 1.6 1.3 1.6-.9 0-1.3.7-1.3 1.6v2c0 1-.4 1.6-1.6 1.6"/></svg>"##;
const SVG_TS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="2" width="12" height="12" rx="2"/><path d="M4.2 6.2h3.4M5.9 6.2v5"/><path d="M11.6 7c-.3-.6-.9-.9-1.6-.9-.9 0-1.5.5-1.5 1.2 0 .8.6 1.1 1.6 1.3 1 .2 1.6.6 1.6 1.4 0 .8-.7 1.3-1.7 1.3-.8 0-1.4-.3-1.8-.9"/></svg>"##;
const SVG_JS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="2" width="12" height="12" rx="2"/><path d="M6.8 6.1v3.7c0 1-.5 1.5-1.4 1.5-.6 0-1-.2-1.3-.7"/><path d="M11.7 7c-.3-.6-.9-.9-1.5-.9-.8 0-1.4.5-1.4 1.2 0 .8.6 1.1 1.5 1.3 1 .2 1.6.6 1.6 1.4 0 .8-.7 1.3-1.6 1.3-.8 0-1.4-.3-1.7-.9"/></svg>"##;
const SVG_PYTHON: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M5.4 5.6V4.1c0-1.1.9-1.9 2.6-1.9s2.6.8 2.6 1.9v2.3c0 1-.8 1.6-1.8 1.6H6.2c-1.1 0-1.9.8-1.9 1.9v1.2c0 1 .7 1.7 1.7 1.7h1.4"/><path d="M10.6 10.4v1.5c0 1.1-.9 1.9-2.6 1.9s-2.6-.8-2.6-1.9V9.6"/><path d="M10.6 5.4h1.3c1 0 1.8.8 1.8 1.8v1.6c0 1-.8 1.8-1.8 1.8H9.2"/><circle cx="7.1" cy="4.2" r="0.7" fill="currentColor" stroke="none"/><circle cx="8.9" cy="11.8" r="0.7" fill="currentColor" stroke="none"/></svg>"##;
const SVG_TERMINAL: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><rect x="1.8" y="2.8" width="12.4" height="10.4" rx="1.8"/><path d="M4.6 6.1L7.1 8 4.6 9.9M8.3 10.2h3.1"/></svg>"##;
const SVG_DATABASE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><ellipse cx="8" cy="4.2" rx="4.8" ry="1.8"/><path d="M3.2 4.2v7.6c0 1 2.1 1.8 4.8 1.8s4.8-.8 4.8-1.8V4.2"/><path d="M3.2 8c0 1 2.1 1.8 4.8 1.8s4.8-.8 4.8-1.8"/></svg>"##;
const SVG_GIT: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M8 1.7L14.3 8 8 14.3 1.7 8z"/><path d="M6.3 4.8l3.2 3.2M8 6.5v3.4"/><circle cx="8" cy="6.5" r="1" fill="currentColor" stroke="none"/><circle cx="8" cy="10.4" r="1" fill="currentColor" stroke="none"/><circle cx="10.2" cy="8.4" r="1" fill="currentColor" stroke="none"/></svg>"##;
const SVG_DOCKER: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"><path d="M2.2 8.7h10.2c-.3 2.5-2.2 4.1-5.1 4.1-2.8 0-4.9-1.5-5.1-4.1z"/><path d="M12.4 8.7c.9-.1 1.5-.6 1.8-1.4-.7-.3-1.4-.1-1.9.3"/><rect x="4" y="6.3" width="2" height="2.1"/><rect x="6.5" y="6.3" width="2" height="2.1"/><rect x="9" y="6.3" width="2" height="2.1"/><rect x="6.5" y="3.8" width="2" height="2.1"/></svg>"##;
const SVG_SETTINGS_SLIDERS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 4.5h11M2.5 8h11M2.5 11.5h11"/><circle cx="5.5" cy="4.5" r="1.4" fill="currentColor" stroke="none"/><circle cx="10.5" cy="8" r="1.4" fill="currentColor" stroke="none"/><circle cx="6.8" cy="11.5" r="1.4" fill="currentColor" stroke="none"/></svg>"##;
const SVG_YAML_LIST: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.35" stroke-linecap="round" stroke-linejoin="round"><path d="M3 4.2h2.6M7.4 4.2h5.6M5 8h2.6M9.4 8h3.6M5 11.8h2.6M9.4 11.8h3.6"/></svg>"##;
const SVG_HASH_CSS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M6.4 2.8L4.8 13.2M11.2 2.8L9.6 13.2M2.8 6h10.4M2.4 10h10.4"/></svg>"##;
const SVG_IMAGE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="2.6" width="12" height="10.8" rx="1.6"/><circle cx="5.6" cy="5.8" r="1.1"/><path d="M2.5 11.8l3.1-3.1 2.2 2.1 2.4-2.8 3.3 3.8"/></svg>"##;
const SVG_ARCHIVE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="2.5" width="11" height="11" rx="1.6"/><path d="M8 2.5v11M6.7 5h1.3M8 7h1.3M6.7 9h1.3M8 11h1.3"/></svg>"##;
const SVG_LOCK: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><rect x="3.2" y="7.2" width="9.6" height="6.4" rx="1.4"/><path d="M5.4 7.2V5.2a2.6 2.6 0 0 1 5.2 0v2"/></svg>"##;
const SVG_SHIELD: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M8 2L3 4v4.1c0 3 2.1 5 5 6 2.9-1 5-3 5-6V4z"/><path d="M5.8 8.1l1.5 1.5 3-3.1"/></svg>"##;
const SVG_DOC_LINES: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round"><path d="M3.6 2h5.4L12.4 5.4V14H3.6z"/><path d="M9 2v3.4h3.4M5.7 8h4.6M5.7 10.6h3.4"/></svg>"##;
const SVG_TABLE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round"><rect x="2.2" y="2.6" width="11.6" height="10.8" rx="1.4"/><path d="M2.2 6.2h11.6M2.2 9.8h11.6M6.4 6.2v7.2"/></svg>"##;
const SVG_VUE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.35" stroke-linecap="round" stroke-linejoin="round"><path d="M2.2 3.2L8 13.2l5.8-10"/><path d="M5.1 3.2L8 8.4l2.9-5.2"/></svg>"##;
const SVG_GO: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.35" stroke-linecap="round" stroke-linejoin="round"><path d="M2 6h2.2M1.5 8.5h2.5M2.6 11h1.8"/><path d="M8.7 5.2a2.9 2.9 0 1 0 1.7 5.2V8.2H8.4"/><circle cx="12.6" cy="8" r="2.2"/></svg>"##;
const SVG_KOTLIN: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.35" stroke-linecap="round" stroke-linejoin="round"><path d="M3 3h10L8 8l5 5H3z"/><path d="M3 13L13 3"/></svg>"##;
const SVG_JAVA: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M3.5 7h7v3.4a3 3 0 0 1-3 3h-1a3 3 0 0 1-3-3z"/><path d="M10.5 7.7h1.4a1.4 1.4 0 0 1 0 2.8h-1.4"/><path d="M5.6 4.8c0-.9.8-1.2.8-2.1M8.2 4.8c0-.9.8-1.2.8-2.1"/></svg>"##;
const SVG_SWIFT: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M9.6 2.6c2.6 2.1 3.7 5.1 2.7 7.6 1 .9 1.3 1.9 1.1 2.8-1.2-.8-2.5-.6-3.7-.1-2.5 1-5.6-.1-7.4-2.8 2.2 1.2 4.5 1.2 6.3.1C6.2 8.7 4.4 6.5 3.3 4.2c1.8 1.5 3.8 2.8 5.6 3.5C7.6 5.9 6.6 4.3 6 2.9c1.4 1.3 3 2.7 4.4 3.6.2-1.4-.1-2.7-.8-3.9z"/></svg>"##;
const SVG_BINARY: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round"><rect x="2.2" y="2.2" width="11.6" height="11.6" rx="1.6"/><path d="M5.5 5.2v2.4M10 5.2c-.8 0-1.2.5-1.2 1.2s.4 1.2 1.2 1.2 1.2-.5 1.2-1.2-.4-1.2-1.2-1.2zM5.5 9.2c-.8 0-1.2.5-1.2 1.2s.4 1.2 1.2 1.2 1.2-.5 1.2-1.2-.4-1.2-1.2-1.2zM10.2 9.2v2.4"/></svg>"##;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct FileIconSpec {
    pub(super) svg: &'static [u8],
    pub(super) color: u32,
}

pub(super) fn file_icon_spec(path: &Path) -> FileIconSpec {
    let c = icon_colors();
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    // Special filenames take priority over generic extensions
    match name.as_str() {
        "cargo.toml" => {
            return FileIconSpec {
                svg: SVG_RUST,
                color: c.rust,
            };
        }
        "cargo.lock" | "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml" | "poetry.lock"
        | "gemfile.lock" | "composer.lock" => {
            return FileIconSpec {
                svg: SVG_LOCK,
                color: c.lock,
            };
        }
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitkeep" | ".ignore" => {
            return FileIconSpec {
                svg: SVG_GIT,
                color: c.git,
            };
        }
        "dockerfile" | ".dockerignore" => {
            return FileIconSpec {
                svg: SVG_DOCKER,
                color: c.docker,
            };
        }
        "license" | "license.md" | "license.txt" | "notice" | "notice.txt" | "copying" => {
            return FileIconSpec {
                svg: SVG_SHIELD,
                color: c.license,
            };
        }
        "makefile" | "cmakelists.txt" | "justfile" => {
            return FileIconSpec {
                svg: SVG_TERMINAL,
                color: c.shell,
            };
        }
        ".env" | ".editorconfig" | ".prettierrc" | ".eslintrc" | ".ds_store" => {
            return FileIconSpec {
                svg: SVG_SETTINGS_SLIDERS,
                color: c.config,
            };
        }
        _ => {}
    }

    if name.starts_with("docker-compose") || name.starts_with("compose.") {
        return FileIconSpec {
            svg: SVG_DOCKER,
            color: c.docker,
        };
    }
    if name.starts_with(".env.") {
        return FileIconSpec {
            svg: SVG_SETTINGS_SLIDERS,
            color: c.config,
        };
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    match ext.as_str() {
        "rs" => FileIconSpec {
            svg: SVG_RUST,
            color: c.rust,
        },
        "ts" | "tsx" | "mts" | "cts" => FileIconSpec {
            svg: SVG_TS,
            color: c.ts,
        },
        "js" | "jsx" | "mjs" | "cjs" => FileIconSpec {
            svg: SVG_JS,
            color: c.js,
        },
        "py" | "pyi" | "ipynb" => FileIconSpec {
            svg: SVG_PYTHON,
            color: c.py,
        },
        "go" => FileIconSpec {
            svg: SVG_GO,
            color: c.go,
        },
        "java" | "jar" => FileIconSpec {
            svg: SVG_JAVA,
            color: c.java,
        },
        "kt" | "kts" | "gradle" => FileIconSpec {
            svg: SVG_KOTLIN,
            color: c.kotlin,
        },
        "swift" => FileIconSpec {
            svg: SVG_SWIFT,
            color: c.swift,
        },
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "m" | "mm" => FileIconSpec {
            svg: SVG_CODE_BRACKETS,
            color: c.c,
        },
        "cs" => FileIconSpec {
            svg: SVG_HASH_CSS,
            color: c.csharp,
        },
        "vue" | "svelte" => FileIconSpec {
            svg: SVG_VUE,
            color: c.vue,
        },
        "html" | "htm" | "xml" | "plist" | "xhtml" => FileIconSpec {
            svg: SVG_CODE_BRACKETS,
            color: c.html,
        },
        "css" | "scss" | "sass" | "less" => FileIconSpec {
            svg: SVG_HASH_CSS,
            color: c.css,
        },
        "md" | "markdown" | "mdx" => FileIconSpec {
            svg: SVG_MARKDOWN,
            color: c.md,
        },
        "json" | "jsonc" | "jsonl" | "geojson" => FileIconSpec {
            svg: SVG_BRACES,
            color: c.json,
        },
        "yml" | "yaml" => FileIconSpec {
            svg: SVG_YAML_LIST,
            color: c.yaml,
        },
        "toml" | "ini" | "conf" | "cfg" | "properties" => FileIconSpec {
            svg: SVG_SETTINGS_SLIDERS,
            color: c.config,
        },
        "sh" | "bash" | "zsh" | "fish" | "bat" | "ps1" | "cmd" => FileIconSpec {
            svg: SVG_TERMINAL,
            color: c.shell,
        },
        "sql" | "sqlite" | "sqlite3" | "db" => FileIconSpec {
            svg: SVG_DATABASE,
            color: c.sql,
        },
        "csv" | "tsv" | "xls" | "xlsx" => FileIconSpec {
            svg: SVG_TABLE,
            color: c.csv,
        },
        "pdf" => FileIconSpec {
            svg: SVG_DOC_LINES,
            color: c.pdf,
        },
        "doc" | "docx" | "txt" | "rtf" | "log" => FileIconSpec {
            svg: SVG_DOC_LINES,
            color: c.doc,
        },
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "icns" | "svg" => FileIconSpec {
            svg: SVG_IMAGE,
            color: c.image,
        },
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "dmg" | "pkg" | "bundle" => {
            FileIconSpec {
                svg: SVG_ARCHIVE,
                color: c.archive,
            }
        }
        "lock" => FileIconSpec {
            svg: SVG_LOCK,
            color: c.lock,
        },
        "wasm" | "bin" | "exe" | "dll" | "so" | "dylib" | "o" | "a" => FileIconSpec {
            svg: SVG_BINARY,
            color: c.binary,
        },
        _ => FileIconSpec {
            svg: SVG_FILE_DEFAULT,
            color: theme().muted,
        },
    }
}

pub(super) fn file_type_icon(path: &Path) -> impl IntoElement {
    let spec = file_icon_spec(path);
    svg()
        .data(spec.svg)
        .size(rems(1.0))
        .flex_shrink_0()
        .text_color(rgb(spec.color))
}

pub(super) fn folder_type_icon(name: &str, expanded: bool) -> impl IntoElement {
    let c = icon_colors();
    let lower = name.to_ascii_lowercase();
    let special = matches!(
        lower.as_str(),
        ".github" | ".git" | "src" | "docs" | "tests" | "test" | "client" | "server"
    );
    let color = if special {
        c.folder_special
    } else if expanded {
        c.folder_open
    } else {
        c.folder
    };
    svg()
        .data(if expanded {
            SVG_FOLDER_OPEN
        } else {
            SVG_FOLDER_CLOSED
        })
        .size(rems(1.0))
        .flex_shrink_0()
        .text_color(rgb(color))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_file_types_to_distinct_specs() {
        let c = icon_colors();
        assert_eq!(file_icon_spec(Path::new("src/main.rs")).color, c.rust);
        assert_eq!(file_icon_spec(Path::new("Cargo.toml")).color, c.rust);
        assert_eq!(file_icon_spec(Path::new("Cargo.lock")).color, c.lock);
        assert_eq!(file_icon_spec(Path::new("AGENTS.md")).color, c.md);
        assert_eq!(file_icon_spec(Path::new("index.html")).color, c.html);
        assert_eq!(file_icon_spec(Path::new(".gitignore")).color, c.git);
        assert_eq!(file_icon_spec(Path::new("LICENSE")).color, c.license);
        assert_eq!(file_icon_spec(Path::new("package.json")).color, c.json);
        assert_eq!(file_icon_spec(Path::new("ci.yml")).color, c.yaml);
        assert_eq!(file_icon_spec(Path::new("logo.png")).color, c.image);
        assert_eq!(file_icon_spec(Path::new("Dockerfile")).color, c.docker);
        assert_eq!(
            file_icon_spec(Path::new("docker-compose.yml")).color,
            c.docker
        );
    }
}
