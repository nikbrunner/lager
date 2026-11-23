//! `lager skill`: the embedded agent guide and the skill stub that points at it.

use std::path::{Path, PathBuf};

use super::args::SkillArgs;
use crate::presentation::escape;

const GUIDE: &str = include_str!("skill/guide.md");
const FILES: [(&str, &str); 2] = [
    ("SKILL.md", include_str!("../../skills/lager/SKILL.md")),
    (
        "references/setup.md",
        include_str!("../../skills/lager/references/setup.md"),
    ),
];

pub(super) fn run(args: SkillArgs) -> i32 {
    if args.command.is_none() {
        print!("{GUIDE}");
        return 0;
    }
    let Some(home) = std::env::var_os("HOME") else {
        eprintln!("error: HOME is not set");
        return 1;
    };
    let target = PathBuf::from(home).join(".agents/skills/lager");
    match install(&target) {
        Ok(()) => {
            println!("Installed skill to {}", escape(target.display()));
            0
        }
        Err(error) => {
            eprintln!("error: {}", escape(error));
            1
        }
    }
}

fn install(target: &Path) -> Result<(), String> {
    for (name, content) in FILES {
        let path = target.join(name);
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().unwrap_or(target))?;
            std::fs::write(&path, content)
        };
        write().map_err(|error| format!("write {}: {error}", path.display()))?;
    }
    Ok(())
}
