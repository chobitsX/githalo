use std::{
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub depth: usize,
    pub expanded: bool,
}

pub fn children(root: &Path, relative: &Path) -> Result<Vec<ProjectEntry>, String> {
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("项目目录路径无效".into());
    }
    let dir = root.join(relative);
    let real = dir
        .canonicalize()
        .map_err(|e| format!("无法访问 {}（{e}）", dir.display()))?;
    if real != dir || !real.starts_with(root) {
        return Err("目录位置已变化，请重新读取工作树".into());
    }
    let listing = fs::read_dir(&real).map_err(|e| format!("无法读取 {}（{e}）", real.display()))?;
    let depth = relative.components().count();
    let mut entries = Vec::new();
    for entry in listing {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let ty = match entry.file_type() {
            Ok(ty) => ty,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.to_string()),
        };
        entries.push(ProjectEntry {
            path: relative.join(&name),
            name: name.to_string_lossy().into_owned(),
            is_dir: ty.is_dir(),
            depth,
            expanded: false,
        });
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.path.cmp(&b.path)));
    Ok(entries)
}
