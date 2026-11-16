//! Source filesystem syntax, independent of the desktop compilation target.
//! No filesystem access, implicit process cwd, case folding or normalization.
use serde::{Deserialize, Serialize};
use typed_path::{Utf8UnixPath, Utf8WindowsComponent, Utf8WindowsPath, Utf8WindowsPrefix};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathFlavor {
    Posix,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathForm {
    Absolute,
    Relative,
    DriveRelative,
    DriveRootRelative,
    Device,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePath {
    pub flavor: PathFlavor,
    pub text: String,
}

impl SourcePath {
    pub fn new(flavor: PathFlavor, text: impl Into<String>) -> Result<Self, &'static str> {
        let text = text.into();
        if text.is_empty() || text.len() > 16384 || text.chars().any(char::is_control) {
            return Err("Invalid source path");
        }
        let valid = match flavor {
            PathFlavor::Posix => Utf8UnixPath::new(&text).is_valid(),
            PathFlavor::Windows => Utf8WindowsPath::new(&text).is_valid(),
        };
        if !valid {
            return Err("Invalid source path components");
        }
        Ok(Self { flavor, text })
    }

    pub fn form(&self) -> PathForm {
        match self.flavor {
            PathFlavor::Posix => {
                if Utf8UnixPath::new(&self.text).is_absolute() {
                    PathForm::Absolute
                } else {
                    PathForm::Relative
                }
            }
            PathFlavor::Windows => {
                let path = Utf8WindowsPath::new(&self.text);
                if matches!(path.components().next(), Some(Utf8WindowsComponent::Prefix(p))
                    if matches!(p.kind(), Utf8WindowsPrefix::DeviceNS(_) | Utf8WindowsPrefix::Verbatim(_)))
                {
                    PathForm::Device
                } else if path.is_absolute() {
                    PathForm::Absolute
                } else if path.has_root() {
                    PathForm::DriveRootRelative
                } else if matches!(
                    path.components().next(),
                    Some(Utf8WindowsComponent::Prefix(_))
                ) {
                    PathForm::DriveRelative
                } else {
                    PathForm::Relative
                }
            }
        }
    }

    /// Join only an explicit source cwd. Preserve `..` for the real filesystem.
    pub fn resolve(&self, cwd: Option<&str>, home: Option<&str>) -> Result<Self, &'static str> {
        if self.text == "~" || self.text.starts_with("~/") {
            let home = Self::new(
                self.flavor,
                home.ok_or("Source account Home is unavailable")?,
            )?;
            if home.form() != PathForm::Absolute {
                return Err("Source account Home is not absolute");
            }
            return match self.text.strip_prefix("~/") {
                Some(tail) => Self::new(self.flavor, tail)?.resolve(Some(&home.text), None),
                None => Ok(home),
            };
        }
        match self.form() {
            PathForm::Absolute => return Ok(self.clone()),
            PathForm::Device => return Err("Device namespaces cannot be opened as ordinary files"),
            PathForm::DriveRelative => {
                return Err("The source drive working directory is required")
            }
            _ => {}
        }
        let cwd = Self::new(
            self.flavor,
            cwd.ok_or("Source working directory is unavailable")?,
        )?;
        if cwd.form() != PathForm::Absolute {
            return Err("Source working directory is not absolute");
        }
        let joined = match self.flavor {
            PathFlavor::Posix => Utf8UnixPath::new(&cwd.text).join(&self.text).to_string(),
            PathFlavor::Windows => Utf8WindowsPath::new(&cwd.text).join(&self.text).to_string(),
        };
        Self::new(self.flavor, joined)
    }
}


