use crate::{Error, Result, sha256};
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Paper {
    pub width_mm: f64,
    pub height_mm: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    #[serde(default = "default_scale", skip_serializing_if = "is_default_scale")]
    pub scale_percent: f64,
    pub alignment: String,
    pub margin_mm: f64,
    pub rotation_deg: u16,
    pub mirror: bool,
    pub offset_x_mm: f64,
    pub offset_y_mm: f64,
}
fn default_scale() -> f64 {
    100.
}
fn is_default_scale(value: &f64) -> bool {
    *value == 100.
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RasterSettings {
    pub mode: String,
    pub threshold: u8,
    pub white_cutoff: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Printer {
    pub density: u8,
    pub speed: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub version: u8,
    pub source_sha256: String,
    pub paper: Paper,
    pub layout: Layout,
    pub raster: RasterSettings,
    pub printer: Printer,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, clap::Args)]
#[serde(default, deny_unknown_fields)]
pub struct Overrides {
    /// Printed artwork size: 100 is contain fit; 10–200 percent.
    #[arg(long, allow_hyphen_values = true)]
    pub scale_percent: Option<f64>,
    #[arg(long)]
    pub width_mm: Option<f64>,
    #[arg(long)]
    pub height_mm: Option<f64>,
    #[arg(long)]
    pub alignment: Option<String>,
    #[arg(long)]
    pub margin_mm: Option<f64>,
    #[arg(long)]
    pub rotation_deg: Option<u16>,
    #[arg(long, action=clap::ArgAction::Set)]
    pub mirror: Option<bool>,
    #[arg(long, allow_hyphen_values = true)]
    pub offset_x_mm: Option<f64>,
    #[arg(long, allow_hyphen_values = true)]
    pub offset_y_mm: Option<f64>,
    #[arg(long)]
    pub raster_mode: Option<String>,
    #[arg(long)]
    pub threshold: Option<u8>,
    #[arg(long)]
    pub white_cutoff: Option<u8>,
    #[arg(long)]
    pub density: Option<u8>,
    #[arg(long)]
    pub speed: Option<u8>,
}
impl Settings {
    pub fn defaults(source: &str, width_mm: f64, height_mm: f64) -> Self {
        Self {
            version: 1,
            source_sha256: source.into(),
            paper: Paper {
                width_mm,
                height_mm,
            },
            layout: Layout {
                scale_percent: 100.,
                alignment: "center".into(),
                margin_mm: 1.,
                rotation_deg: 0,
                mirror: false,
                offset_x_mm: 0.,
                offset_y_mm: 0.,
            },
            raster: RasterSettings {
                mode: "threshold".into(),
                threshold: 128,
                white_cutoff: 255,
            },
            printer: Printer {
                density: 10,
                speed: 1,
            },
        }
    }
    pub fn validate(&mut self, source: &str) -> Result<()> {
        let within = |x: f64, a: f64, b: f64| x.is_finite() && (a..=b).contains(&x);
        if self.version != 1
            || self.source_sha256.len() != 64
            || !self.source_sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(Error::localized(
                "invalid_settings",
                "err.settingsFormat",
                &[],
            ));
        }
        if self.source_sha256 != source {
            return Err(Error::localized(
                "settings_source_mismatch",
                "err.sourceChanged",
                &[],
            ));
        }
        if !within(self.paper.width_mm, 20., 50.)
            || !within(self.layout.scale_percent, 10., 200.)
            || !within(self.paper.height_mm, 10., 100.)
            || !within(self.layout.margin_mm, 0., 5.)
            || self.layout.margin_mm * 2. >= self.paper.height_mm
            || !within(self.layout.offset_x_mm, -10., 10.)
            || !within(self.layout.offset_y_mm, -10., 10.)
            || ![0, 90, 180, 270].contains(&self.layout.rotation_deg)
            || !["left", "center", "right"].contains(&self.layout.alignment.as_str())
            || !["threshold", "floyd-steinberg"].contains(&self.raster.mode.as_str())
            || !(1..=15).contains(&self.printer.density)
            || !(1..=5).contains(&self.printer.speed)
        {
            return Err(Error::localized(
                "invalid_settings",
                "err.settingsRange",
                &[],
            ));
        }
        if self.raster.mode == "floyd-steinberg" {
            self.raster.threshold = 128;
        }
        Ok(())
    }
    pub fn apply(&mut self, o: &Overrides) {
        macro_rules! set { ($($a:ident.$b:ident => $c:ident),* $(,)?) => {$(if let Some(v)=&o.$c {self.$a.$b=v.clone();})*}; }
        set!(paper.width_mm=>width_mm,paper.height_mm=>height_mm,layout.scale_percent=>scale_percent,layout.alignment=>alignment,layout.margin_mm=>margin_mm,layout.rotation_deg=>rotation_deg,layout.mirror=>mirror,layout.offset_x_mm=>offset_x_mm,layout.offset_y_mm=>offset_y_mm,raster.mode=>raster_mode,raster.threshold=>threshold,raster.white_cutoff=>white_cutoff,printer.density=>density,printer.speed=>speed);
    }
}
impl Overrides {
    pub fn validate_finite(&self) -> Result<()> {
        if [
            self.scale_percent,
            self.width_mm,
            self.height_mm,
            self.margin_mm,
            self.offset_x_mm,
            self.offset_y_mm,
        ]
        .into_iter()
        .flatten()
        .any(|v| !v.is_finite())
        {
            return Err(Error::localized(
                "invalid_settings",
                "err.finiteNumber",
                &[],
            ));
        }
        Ok(())
    }
}
pub fn dots(mm: f64) -> i32 {
    (mm.abs() * 8. + 0.5).floor() as i32 * if mm < 0. { -1 } else { 1 }
}
pub fn bounded_read(path: &Path, limit: usize, code: &str) -> Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|e| Error::new(code, format!("{}: {e}", path.display())))?;
    if !file
        .metadata()
        .map_err(|e| Error::new(code, e.to_string()))?
        .is_file()
    {
        return Err(Error::localized(code, "err.regularFile", &[]));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::new(code, e.to_string()))?;
    if bytes.len() > limit {
        return Err(Error::localized(
            code,
            "err.fileLimit",
            &[("limit", limit.to_string())],
        ));
    }
    Ok(bytes)
}
pub fn sidecar(path: &Path) -> PathBuf {
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
    {
        path.with_extension("openlabel.json")
    } else {
        let mut name = path.as_os_str().to_os_string();
        name.push(".openlabel-image.json");
        PathBuf::from(name)
    }
}
pub fn input_hash(source: &[u8], settings: Option<&[u8]>) -> String {
    let mut bytes = b"openlabel-input-v1\0".to_vec();
    bytes.extend_from_slice(&(source.len() as u64).to_le_bytes());
    bytes.extend_from_slice(source);
    bytes.push(u8::from(settings.is_some()));
    if let Some(s) = settings {
        bytes.extend_from_slice(&(s.len() as u64).to_le_bytes());
        bytes.extend_from_slice(s);
    }
    sha256(&bytes)
}
pub fn write_output(
    path: &Path,
    bytes: &[u8],
    overwrite: bool,
    protected: &[PathBuf],
) -> Result<()> {
    let target = match std::fs::symlink_metadata(path) {
        Ok(meta) => Some(meta),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(Error::new("output_error", e.to_string())),
    };
    if let Some(target) = target {
        if !overwrite {
            return Err(Error::localized("output_exists", "err.outputExists", &[]));
        }
        if !target.is_file() {
            return Err(Error::localized("output_error", "err.outputRegular", &[]));
        }
        for input in protected {
            if let Ok(meta) = std::fs::metadata(input) {
                #[cfg(unix)]
                let same = {
                    use std::os::unix::fs::MetadataExt;
                    meta.dev() == target.dev() && meta.ino() == target.ino()
                };
                #[cfg(not(unix))]
                let same = std::fs::canonicalize(input).ok() == std::fs::canonicalize(path).ok();
                if same {
                    return Err(Error::localized("output_error", "err.outputProtected", &[]));
                }
            }
        }
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| Error::new("output_error", e.to_string()))?;
    staged
        .write_all(bytes)
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|e| Error::new("output_error", e.to_string()))?;
    let result = if overwrite {
        staged.persist(path)
    } else {
        staged.persist_noclobber(path)
    };
    result.map(|_| ()).map_err(|e| {
        Error::new(
            if e.error.kind() == std::io::ErrorKind::AlreadyExists {
                "output_exists"
            } else {
                "output_error"
            },
            e.error.to_string(),
        )
    })
}
