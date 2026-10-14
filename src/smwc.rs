//! API de SMWCentral, descarga y parcheo.

use serde::{Deserialize, Serialize};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

const API: &str = "https://www.smwcentral.net/ajax.php";
const UA: &str = concat!("smw-launcher/", env!("CARGO_PKG_VERSION"));
/// CRC32 de Super Mario World (USA) sin header.
pub const SMW_CRC: u32 = 0xB19E_D489;

pub const DIFFICULTIES: [&str; 8] =
    ["Cualquiera", "Newcomer", "Casual", "Intermediate", "Advanced", "Expert", "Master", "Grandmaster"];
pub const KINDS: [(&str, &str); 6] = [
    ("Cualquiera", ""),
    ("Standard", "standard"),
    ("Kaizo", "kaizo"),
    ("Puzzle", "puzzle"),
    ("Tool-Assisted", "tool_assisted"),
    ("Pit", "pit"),
];
/// Valores de orden de la API (las etiquetas traducidas están en la UI).
pub const SORTS: [&str; 4] = ["date", "downloads", "rating", "name"];

#[derive(Deserialize, Serialize, Clone, Default)]
pub struct Hack {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub authors: Vec<Author>,
    #[serde(default)]
    pub images: Vec<String>,
    pub rating: Option<f32>,
    #[serde(default)]
    pub downloads: u64,
    pub download_url: String,
    pub fields: Fields,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct Author {
    pub name: String,
}

#[derive(Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct Fields {
    pub version: Option<String>,
    pub difficulty: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub length: Option<String>,
    pub description: Option<String>,
}

impl Hack {
    pub fn authors(&self) -> String {
        self.authors.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
    }
    /// Descripción en texto plano (la API la manda con HTML).
    pub fn description(&self) -> String {
        let html = self.fields.description.as_deref().unwrap_or("");
        let (mut out, mut tag, mut in_tag) = (String::new(), String::new(), false);
        for c in html.chars() {
            match c {
                '<' => (in_tag, tag) = (true, String::new()),
                '>' if in_tag => {
                    in_tag = false;
                    if matches!(tag.trim_end_matches('/').trim().to_lowercase().as_str(), "br" | "p" | "/p" | "li") {
                        out.push('\n');
                    }
                }
                _ if in_tag => tag.push(c),
                _ => out.push(c),
            }
        }
        [("&amp;", "&"), ("&quot;", "\""), ("&#039;", "'"), ("&#39;", "'"), ("&lt;", "<"), ("&gt;", ">"), ("&nbsp;", " ")]
            .iter()
            .fold(out, |s, (a, b)| s.replace(a, b))
    }

    pub fn url(&self) -> String {
        format!("https://www.smwcentral.net/?p=section&a=details&id={}", self.id)
    }
}

#[derive(Deserialize)]
pub struct Page {
    pub data: Vec<Hack>,
    pub total: u32,
    pub last_page: u32,
}

#[derive(Default, Clone)]
pub struct Query {
    pub name: String,
    pub difficulty: usize,
    pub kind: usize,
    pub sort: usize,
    pub page: u32,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub fn search(q: &Query) -> Result<Page, String> {
    let sort = SORTS[q.sort];
    let mut req = ureq::get(API)
        .header("User-Agent", UA)
        .query("a", "getsectionlist")
        .query("s", "smwhacks")
        .query("n", q.page.max(1).to_string())
        .query("o", sort)
        .query("d", if sort == "name" { "asc" } else { "desc" });
    if !q.name.trim().is_empty() {
        req = req.query("f[name]", q.name.trim());
    }
    if q.difficulty > 0 {
        req = req.query("f[difficulty][]", format!("diff_{}", q.difficulty));
    }
    if q.kind > 0 {
        req = req.query("f[type][]", KINDS[q.kind].1);
    }
    req.call().map_err(err)?.body_mut().read_json().map_err(err)
}

/// Lee la ROM limpia quitando el header de copiador de 512 bytes si lo tiene.
pub fn clean_rom(path: &Path) -> Result<Vec<u8>, String> {
    let rom = std::fs::read(path).map_err(|e| format!("{}: {e}", crate::t("No se pudo leer la ROM", "Could not read the ROM")))?;
    Ok(if rom.len() % 1024 == 512 { rom[512..].to_vec() } else { rom })
}

pub fn is_clean_smw(path: &Path) -> bool {
    clean_rom(path).is_ok_and(|r| crc32fast::hash(&r) == SMW_CRC)
}

/// Descarga el zip del hack, aplica su parche y guarda el .sfc en `dest`.
pub fn install(hack: &Hack, rom: &Path, dest: &Path) -> Result<PathBuf, String> {
    const MAX: u64 = 64 << 20;
    let zip = ureq::get(&hack.download_url)
        .header("User-Agent", UA)
        .call()
        .map_err(err)?
        .body_mut()
        .with_config()
        .limit(MAX)
        .read_to_vec()
        .map_err(err)?;
    let mut ar = zip::ZipArchive::new(Cursor::new(zip)).map_err(err)?;
    // ponytail: elige el .bps (o .ips) más grande; hacks con varias variantes no dejan escoger
    let mut best: Option<(bool, u64, usize)> = None;
    for i in 0..ar.len() {
        let f = ar.by_index(i).map_err(err)?;
        let n = f.name().map_err(err)?.to_lowercase();
        let bps = n.ends_with(".bps");
        if (bps || n.ends_with(".ips")) && best.is_none_or(|b| (bps, f.size()) > (b.0, b.1)) {
            best = Some((bps, f.size(), i));
        }
    }
    let (_, _, i) = best.ok_or(crate::t("El zip no contiene ningún parche .bps/.ips", "The zip has no .bps/.ips patch"))?;
    let mut patch = Vec::new();
    ar.by_index(i).map_err(err)?.take(MAX).read_to_end(&mut patch).map_err(err)?;
    let out = crate::patch::apply(&patch, &clean_rom(rom)?)?;
    std::fs::create_dir_all(dest).map_err(err)?;
    let name: String =
        hack.name.chars().map(|c| if c.is_alphanumeric() || " -_()'.!&,".contains(c) { c } else { '_' }).collect();
    let path = dest.join(format!("{}.sfc", name.trim_start_matches('.')));
    std::fs::write(&path, out).map_err(err)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    #[test]
    fn description_strips_html() {
        let mut h: super::Hack = serde_json::from_str(r#"{"id":1,"name":"x","download_url":"","fields":{}}"#).unwrap();
        h.fields.description = Some("Hi<br>see <a href=\"x\">this</a> &amp; that<br />".into());
        assert_eq!(h.description(), "Hi\nsee this & that\n");
    }

    /// Prueba real contra SMWCentral: `SMW_ROM=/ruta/smw.sfc cargo test -- --ignored`
    #[test]
    #[ignore]
    fn search_and_install() {
        let rom = std::env::var("SMW_ROM").expect("define SMW_ROM");
        assert!(super::is_clean_smw(rom.as_ref()));
        let q = super::Query { name: "bowser's return redux".into(), page: 1, ..Default::default() };
        let hack = super::search(&q).unwrap().data.remove(0);
        let dest = std::env::temp_dir().join("smw-launcher-test");
        let out = super::install(&hack, rom.as_ref(), &dest).unwrap();
        assert!(std::fs::metadata(&out).unwrap().len() >= 512 * 1024);
        std::fs::remove_dir_all(dest).ok();
    }
}
