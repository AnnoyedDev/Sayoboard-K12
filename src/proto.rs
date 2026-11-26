use hidapi::{HidApi, HidDevice};

pub const VID: u16 = 0x8089;
const REPORT_ID: u8 = 0x02;
const TIMEOUT_MS: i32 = 1000;

pub const CMD_INFO: u8 = 0x00;
pub const CMD_SAVE: u8 = 0x04;
pub const CMD_LED: u8 = 0x10;
pub const CMD_KEY: u8 = 0x16;
pub const SILENT: u8 = 0xC0;

pub fn frame(cmd: u8, payload: &[u8]) -> [u8; 64] {
    assert!(payload.len() <= 60);
    let mut f = [0u8; 64];
    f[0] = REPORT_ID;
    f[1] = cmd;
    f[2] = payload.len() as u8;
    f[3..3 + payload.len()].copy_from_slice(payload);
    let n = 3 + payload.len();
    f[n] = f[..n].iter().fold(0u8, |a, b| a.wrapping_add(*b));
    f
}

pub struct Device {
    dev: HidDevice,
    pub name: String,
}

pub struct Info {
    pub version: u16,
    pub model: u16,
}

#[derive(Clone)]
pub struct Key {
    pub raw: Vec<u8>,
}

impl Key {
    pub fn number(&self) -> u8 {
        self.raw[1]
    }
    pub fn layers(&self) -> usize {
        self.raw.len().saturating_sub(16) / 6
    }
    pub fn layer(&mut self, i: usize) -> &mut [u8] {
        &mut self.raw[16 + i * 6..16 + i * 6 + 6]
    }
    pub fn site(&self) -> (u16, u16) {
        (
            u16::from_le_bytes([self.raw[4], self.raw[5]]),
            u16::from_le_bytes([self.raw[6], self.raw[7]]),
        )
    }
}

#[derive(Clone)]
pub struct Led {
    pub raw: Vec<u8>,
}

impl Led {
    pub fn number(&self) -> u8 {
        self.raw[1]
    }
    pub fn color(&mut self) -> &mut [u8] {
        &mut self.raw[6..9]
    }
}

impl Device {
    pub fn open() -> Result<Self, String> {
        let api = HidApi::new().map_err(|e| e.to_string())?;
        let info = api
            .device_list()
            .find(|d| d.vendor_id() == VID && d.usage_page() == 0xFF00)
            .ok_or(t!(
                "Aucun SayoDevice trouvé (VID 8089, page 0xFF00). Règle udev installée ?",
                "No SayoDevice found (VID 8089, page 0xFF00). Is the udev rule installed?",
                "SayoDevice が見つかりません(VID 8089、ページ 0xFF00)。udev ルールはインストール済みですか?"
            ))?;
        let dev = info.open_device(&api).map_err(|e| {
            t!(
                format!("{e} (permissions hidraw ?)"),
                format!("{e} (hidraw permissions?)"),
                format!("{e}(hidraw の権限?)"),
            )
        })?;
        let name = info.product_string().unwrap_or("SayoDevice").to_string();
        Ok(Self { dev, name })
    }

    pub fn transfer(&self, cmd: u8, payload: &[u8]) -> Result<Option<Vec<u8>>, String> {
        let mut buf = [0u8; 64];
        while self
            .dev
            .read_timeout(&mut buf, 0)
            .map_err(|e| e.to_string())?
            > 0
        {}
        self.dev
            .write(&frame(cmd, payload))
            .map_err(|e| e.to_string())?;
        loop {
            let n = self
                .dev
                .read_timeout(&mut buf, TIMEOUT_MS)
                .map_err(|e| e.to_string())?;
            if n == 0 {
                return Err(t!(
                    "Pas de réponse du périphérique",
                    "No response from the device",
                    "デバイスから応答がありません"
                )
                .into());
            }
            if buf[0] != REPORT_ID {
                continue;
            }
            if buf[1] != 0 {
                return Ok(None);
            }
            let len = (buf[2] as usize).min(60);
            return Ok(Some(buf[3..3 + len].to_vec()));
        }
    }

    pub fn info(&self) -> Result<Info, String> {
        let p = self.transfer(CMD_INFO, &[0, 0, 0, 0])?.ok_or(t!(
            "Info refusée",
            "Info request refused",
            "情報の取得が拒否されました"
        ))?;
        let g = |i: usize| *p.get(i).unwrap_or(&0) as u16;
        Ok(Info {
            version: g(0) << 8 | g(1),
            model: g(2) << 8 | g(3),
        })
    }

    fn read_all(&self, cmd: u8) -> Result<Vec<Vec<u8>>, String> {
        let mut out = Vec::new();
        for n in 0..=255u8 {
            match self.transfer(cmd, &[0, n])? {
                Some(p) if p.len() > 2 => out.push(p),
                _ => break,
            }
        }
        Ok(out)
    }

    pub fn keys(&self) -> Result<Vec<Key>, String> {
        Ok(self
            .read_all(CMD_KEY)?
            .into_iter()
            .filter(|p| p.len() >= 22)
            .map(|raw| Key { raw })
            .collect())
    }

    pub fn leds(&self) -> Result<Vec<Led>, String> {
        Ok(self
            .read_all(CMD_LED)?
            .into_iter()
            .filter(|p| p.len() >= 12)
            .map(|raw| Led { raw })
            .collect())
    }

    fn write_raw(&self, cmd: u8, raw: &[u8]) -> Result<Vec<u8>, String> {
        let mut p = raw.to_vec();
        p[0] = 1;
        self.transfer(cmd, &p)?.ok_or_else(|| {
            t!(
                "Écriture refusée par le périphérique",
                "Write refused by the device",
                "デバイスが書き込みを拒否しました"
            )
            .into()
        })
    }

    pub fn write_key(&self, k: &Key) -> Result<Key, String> {
        self.write_raw(CMD_KEY, &k.raw).map(|raw| Key { raw })
    }

    pub fn write_led(&self, l: &Led) -> Result<Led, String> {
        self.write_raw(CMD_LED, &l.raw).map(|raw| Led { raw })
    }

    pub fn save(&self) -> Result<(), String> {
        self.transfer(CMD_SAVE, &[0x72, 0x96])?
            .map(|_| ())
            .ok_or_else(|| t!("Sauvegarde refusée", "Save refused", "保存が拒否されました").into())
    }
}

pub const MODS: [&str; 8] = [
    "LCtrl", "LShift", "LAlt", "LGui", "RCtrl", "RShift", "RAlt", "RGui",
];

pub fn key_name(c: u8) -> String {
    const NAMES: [[&str; 42]; 2] = [
        [
            "Entrée",
            "Échap",
            "Retour",
            "Tab",
            "Espace",
            "-",
            "=",
            "[",
            "]",
            "\\",
            "#",
            ";",
            "'",
            "`",
            ",",
            ".",
            "/",
            "VerrMaj",
            "F1",
            "F2",
            "F3",
            "F4",
            "F5",
            "F6",
            "F7",
            "F8",
            "F9",
            "F10",
            "F11",
            "F12",
            "ImprÉc",
            "ArrêtDéfil",
            "Pause",
            "Inser",
            "Début",
            "PgPréc",
            "Suppr",
            "Fin",
            "PgSuiv",
            "→",
            "←",
            "↓",
        ],
        [
            "Enter",
            "Esc",
            "Backspace",
            "Tab",
            "Space",
            "-",
            "=",
            "[",
            "]",
            "\\",
            "#",
            ";",
            "'",
            "`",
            ",",
            ".",
            "/",
            "CapsLock",
            "F1",
            "F2",
            "F3",
            "F4",
            "F5",
            "F6",
            "F7",
            "F8",
            "F9",
            "F10",
            "F11",
            "F12",
            "PrtSc",
            "ScrLk",
            "Pause",
            "Ins",
            "Home",
            "PgUp",
            "Del",
            "End",
            "PgDn",
            "→",
            "←",
            "↓",
        ],
    ];
    const PAD: [&str; 16] = [
        "/", "*", "-", "+", "↵", "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", ".",
    ];
    match c {
        0 => "—".into(),
        0x04..=0x1D => ((b'A' + c - 4) as char).to_string(),
        0x1E..=0x26 => ((b'1' + c - 0x1E) as char).to_string(),
        0x27 => "0".into(),
        0x28..=0x51 => NAMES[t!(0, 1, 1)][(c - 0x28) as usize].into(),
        0x52 => "↑".into(),
        0x53 => t!("VerrNum", "NumLock", "NumLock").into(),
        0x54..=0x63 => format!(
            "{} {}",
            t!("Pavé", "Num", "テンキー"),
            PAD[(c - 0x54) as usize]
        ),
        0x65 => "Menu".into(),
        0x68..=0x73 => format!("F{}", c - 0x68 + 13),
        SILENT..=0xCB => format!("🔊 {} #{}", t!("son", "sound", "サウンド"), c - SILENT),
        _ => format!("0x{c:02X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_matches_reference() {
        let f = frame(CMD_SAVE, &[0x72, 0x96]);
        assert_eq!(&f[..6], &[0x02, 0x04, 0x02, 0x72, 0x96, 0x10]);
        assert!(f[6..].iter().all(|b| *b == 0));
    }

    #[test]
    fn key_names() {
        assert_eq!(key_name(0x04), "A");
        assert_eq!(key_name(0x27), "0");
        assert_eq!(key_name(0x44), "F11");
        assert_eq!(key_name(0x45), "F12");
        assert_eq!(key_name(0x52), "↑");
        assert_eq!(key_name(0x68), "F13");
        assert_eq!(key_name(0x73), "F24");
        assert_eq!(key_name(SILENT + 11), "🔊 son #11");
    }
}
