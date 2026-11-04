//! Minimal macOS application bundle creation for release artifacts.

use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BundlerError {
    #[error("could not create app bundle directory '{path}': {source}")]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not copy GUI executable '{source_path}' to '{destination}': {source}")]
    Copy {
        source_path: PathBuf,
        destination: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not write '{path}': {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("GUI executable '{0}' does not exist")]
    MissingExecutable(PathBuf),
}

pub fn bundle_app(
    gui_binary: &Path,
    output_dir: &Path,
    version: &str,
) -> Result<PathBuf, BundlerError> {
    if !gui_binary.is_file() {
        return Err(BundlerError::MissingExecutable(gui_binary.to_path_buf()));
    }
    let app_dir = output_dir.join("rust-dl.app");
    let contents = app_dir.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    for directory in [&macos, &resources] {
        fs::create_dir_all(directory).map_err(|source| BundlerError::Create {
            path: directory.to_path_buf(),
            source,
        })?;
    }

    let executable = macos.join("rust-dl");
    fs::copy(gui_binary, &executable).map_err(|source| BundlerError::Copy {
        source_path: gui_binary.to_path_buf(),
        destination: executable.clone(),
        source,
    })?;
    set_executable(&executable)?;

    let plist_path = contents.join("Info.plist");
    fs::write(&plist_path, info_plist(version)).map_err(|source| BundlerError::Write {
        path: plist_path,
        source,
    })?;

    #[cfg(target_os = "macos")]
    {
        // Deep ad-hoc code sign the entire bundle to bind Info.plist and satisfy Gatekeeper
        let _ = std::process::Command::new("codesign")
            .arg("--force")
            .arg("--deep")
            .arg("--sign")
            .arg("-")
            .arg(&app_dir)
            .output();
    }

    Ok(app_dir)
}

fn info_plist(version: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
\t<key>CFBundleName</key>\n\
\t<string>rust-dl</string>\n\
\t<key>CFBundleDisplayName</key>\n\
\t<string>rust-dl</string>\n\
\t<key>CFBundleIdentifier</key>\n\
\t<string>com.opensource.rustdl</string>\n\
\t<key>CFBundleVersion</key>\n\
\t<string>{version}</string>\n\
\t<key>CFBundleShortVersionString</key>\n\
\t<string>{version}</string>\n\
\t<key>CFBundlePackageType</key>\n\
\t<string>APPL</string>\n\
\t<key>CFBundleExecutable</key>\n\
\t<string>rust-dl</string>\n\
\t<key>LSMinimumSystemVersion</key>\n\
\t<string>11.0</string>\n\
\t<key>NSHighResolutionCapable</key>\n\
\t<true/>\n\
\t<key>NSSupportsAutomaticGraphicsSwitching</key>\n\
\t<true/>\n\
\t<key>NSRequiresAquaSystemAppearance</key>\n\
\t<false/>\n\
\t<key>LSApplicationCategoryType</key>\n\
\t<string>public.app-category.utilities</string>\n\
\t<key>NSPrincipalClass</key>\n\
\t<string>NSApplication</string>\n\
</dict>\n\
</plist>\n"
    )
}

fn set_executable(path: &Path) -> Result<(), BundlerError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = fs::Permissions::from_mode(0o755);
        fs::set_permissions(path, permissions).map_err(|source| BundlerError::Write {
            path: path.to_path_buf(),
            source,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_valid_app_bundle_tree_and_plist() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("rust-dl-bundle-{}", std::process::id()));
        match fs::remove_dir_all(&root) {
            Ok(()) | Err(_) => {}
        }
        fs::create_dir_all(&root)?;
        let binary = root.join("rust-dl-gui");
        fs::write(&binary, b"fake executable")?;
        let app = bundle_app(&binary, &root, "1.2.3")?;
        assert!(app.join("Contents/MacOS/rust-dl").is_file());
        assert!(app.join("Contents/Resources").is_dir());
        let plist = fs::read_to_string(app.join("Contents/Info.plist"))?;
        assert!(plist.contains("com.opensource.rustdl"));
        assert!(plist.contains("<string>APPL</string>"));
        assert!(plist.contains("<true/>") || plist.contains("<true />"));
        match fs::remove_dir_all(&root) {
            Ok(()) | Err(_) => {}
        }
        Ok(())
    }
}
