use std::collections::HashSet;
use std::io;

use serde::{Deserialize, Serialize};

pub const MAX_MANIFEST: usize = 64 * 1024;
pub const MAX_ASSET: usize = 192 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    pub version: u32,
    pub default_page: String,
    pub pages: Vec<Page>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Page {
    pub id: String,
    pub body: Block,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TextRole {
    Title,
    Heading,
    Value,
    #[default]
    Body,
    Caption,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Fit {
    #[default]
    Contain,
    Cover,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ButtonStyle {
    Primary,
    #[default]
    Secondary,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ButtonSize {
    #[default]
    Normal,
    Large,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Action {
    Refresh,
    OpenPage { page: String },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Block {
    Column {
        #[serde(default = "default_gap")]
        gap: u16,
        children: Vec<Block>,
    },
    Row {
        #[serde(default = "default_gap")]
        gap: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weights: Option<Vec<u16>>,
        children: Vec<Block>,
    },
    Text {
        text: String,
        #[serde(default)]
        role: TextRole,
        #[serde(default)]
        align: Align,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_lines: Option<u8>,
    },
    Image {
        src: String,
        alt: String,
        #[serde(default = "square")]
        aspect: f32,
        #[serde(default)]
        fit: Fit,
    },
    Button {
        id: String,
        label: String,
        #[serde(default)]
        style: ButtonStyle,
        #[serde(default)]
        size: ButtonSize,
        action: Action,
    },
}

fn default_gap() -> u16 { 16 }
fn square() -> f32 { 1.0 }

pub fn asset_name(src: &str) -> Option<(&str, &str)> {
    let name = src.strip_prefix("/assets/")?;
    let (hash, extension) = name.split_once('.')?;
    if hash.len() == 64
        && hash.bytes().all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        && matches!(extension, "png" | "jpg")
    {
        Some((hash, extension))
    } else {
        None
    }
}

impl Document {
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > MAX_MANIFEST {
            return Err(invalid("dashboard response exceeds 64 KiB"));
        }
        let document: Self = serde_json::from_slice(bytes).map_err(invalid)?;
        document.validate()?;
        Ok(document)
    }

    pub fn page(&self, id: &str) -> Option<&Page> {
        self.pages.iter().find(|page| page.id == id)
    }

    pub fn validate(&self) -> io::Result<()> {
        if self.version != 1 || !(1..=6).contains(&self.pages.len()) {
            return Err(invalid("unsupported version or page count"));
        }
        let ids: HashSet<&str> = self.pages.iter().map(|page| page.id.as_str()).collect();
        if ids.len() != self.pages.len() || !ids.contains(self.default_page.as_str()) || !ids.iter().all(|id| named(id)) {
            return Err(invalid("invalid or duplicate page ID"));
        }
        let mut assets = HashSet::new();
        let mut count = 0;
        for page in &self.pages {
            page.body.validate(1, &mut count, &ids, &mut assets, &mut HashSet::new())?;
        }
        Ok(())
    }
}

impl Block {
    fn validate<'a>(
        &'a self,
        depth: u8,
        count: &mut usize,
        pages: &HashSet<&str>,
        assets: &mut HashSet<&'a str>,
        buttons: &mut HashSet<&'a str>,
    ) -> io::Result<()> {
        *count += 1;
        if depth > 6 || *count > 80 {
            return Err(invalid("too many blocks or nested containers"));
        }
        match self {
            Self::Column { gap, children } | Self::Row { gap, children, .. } => {
                let maximum = if matches!(self, Self::Row { .. }) { 4 } else { 12 };
                if *gap > 48 || !(1..=maximum).contains(&children.len()) {
                    return Err(invalid("invalid container size or spacing"));
                }
                if let Self::Row { weights: Some(weights), .. } = self
                    && (weights.len() != children.len() || weights.iter().any(|weight| !(1..=10).contains(weight)))
                {
                    return Err(invalid("invalid row weights"));
                }
                for child in children {
                    child.validate(depth + 1, count, pages, assets, buttons)?;
                }
            }
            Self::Text { text, max_lines, .. } => {
                if !sized(text, 600) || max_lines.is_some_and(|lines| !(1..=6).contains(&lines)) {
                    return Err(invalid("invalid text size"));
                }
            }
            Self::Image { src, alt, aspect, .. } => {
                assets.insert(src);
                if asset_name(src).is_none() || !sized(alt, 100) || !(0.5..=4.0).contains(aspect) || assets.len() > 6 {
                    return Err(invalid("invalid image or too many images"));
                }
            }
            Self::Button { id, label, action, .. } => {
                if !named(id) || !buttons.insert(id) || !sized(label, 30) {
                    return Err(invalid("invalid or duplicate button"));
                }
                if let Action::OpenPage { page } = action
                    && !pages.contains(page.as_str())
                {
                    return Err(invalid("button points to an unknown page"));
                }
            }
        }
        Ok(())
    }

    pub fn image_sources(&self, output: &mut Vec<String>) {
        match self {
            Self::Column { children, .. } | Self::Row { children, .. } => {
                for child in children {
                    child.image_sources(output);
                }
            }
            Self::Image { src, .. } if !output.contains(src) => output.push(src.clone()),
            _ => (),
        }
    }
}

fn named(value: &str) -> bool {
    (1..=40).contains(&value.len())
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn sized(value: &str, max: usize) -> bool {
    !value.is_empty() && value.chars().count() <= max
}

pub fn invalid(message: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}
