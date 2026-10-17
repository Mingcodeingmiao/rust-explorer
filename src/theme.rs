#[derive(Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub background: u32,
    pub chrome: u32,
    pub panel: u32,
    pub input: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub hover: u32,
    pub selection: u32,
    pub accent: u32,
}
impl Theme {
    pub fn preference(preference: &str) -> Self {
        match preference {
            "light" => Self::with_dark(false),
            "dark" => Self::with_dark(true),
            _ => Self::system(),
        }
    }
    pub fn system() -> Self {
        let dark = windows_registry::CURRENT_USER
            .open("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
            .and_then(|k| k.get_u32("AppsUseLightTheme"))
            .unwrap_or(1)
            == 0;
        Self::with_dark(dark)
    }
    fn with_dark(dark: bool) -> Self {
        if dark {
            Self {
                dark,
                background: 0x191919,
                chrome: 0x202020,
                panel: 0x202020,
                input: 0x303030,
                border: 0x333333,
                text: 0xf2f2f2,
                muted: 0xaaaaaa,
                hover: 0x2b2b2b,
                selection: 0x333d49,
                accent: 0x76b9ed,
            }
        } else {
            Self {
                dark,
                background: 0xffffff,
                chrome: 0xf3f3f3,
                panel: 0xf8f8f8,
                input: 0xffffff,
                border: 0xe5e5e5,
                text: 0x202020,
                muted: 0x666666,
                hover: 0xeeeeee,
                selection: 0xdcecf9,
                accent: 0x0067c0,
            }
        }
    }
}
