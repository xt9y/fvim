use mlua::{Lua, Table, Value};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::OnceLock;

pub const DEFAULTS: &str = include_str!("../config/pre_configured.lua");
pub const INIT: &str = include_str!("../config/init.lua");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Highlight {
    pub fg: Option<(u8, u8, u8)>,
    pub bg: Option<(u8, u8, u8)>,
    pub ctermfg: Option<u8>,
    pub ctermbg: Option<u8>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
}

impl Highlight {
    pub fn over(self, base: Self) -> Self {
        Self {
            fg: self.fg.or(base.fg),
            bg: self.bg.or(base.bg),
            ctermfg: self.ctermfg.or(if self.fg.is_none() {
                base.ctermfg
            } else {
                None
            }),
            ctermbg: self.ctermbg.or(if self.bg.is_none() {
                base.ctermbg
            } else {
                None
            }),
            ..self
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub number: bool,
    pub relativenumber: bool,
    pub numberwidth: usize,
    pub signcolumn: bool,
    pub tabstop: usize,
    pub shiftwidth: usize,
    pub expandtab: bool,
    pub smartindent: bool,
    pub termguicolors: bool,
    pub wrap: bool,
    pub linebreak: bool,
    pub scrolloff: usize,
    pub sidescroll: usize,
    pub sidescrolloff: usize,
    pub laststatus: usize,
    pub cmdheight: usize,
    pub showmode: bool,
    pub statusline: String,
    pub eob: String,
    pub normal_cursor: String,
    pub insert_cursor: String,
    pub output_buffer_size: usize,
    pub highlights: HashMap<String, Highlight>,
}

impl Default for Settings {
    fn default() -> Self {
        static DEFAULT: OnceLock<Settings> = OnceLock::new();
        DEFAULT
            .get_or_init(|| {
                Config::from_scripts(DEFAULTS, "")
                    .expect("bundled config")
                    .settings
            })
            .clone()
    }
}

impl Settings {
    pub fn highlight(&self, group: &str) -> Highlight {
        let normal = self.highlights.get("Normal").copied().unwrap_or_default();
        self.highlights
            .get(group)
            .copied()
            .unwrap_or_default()
            .over(normal)
    }
}

pub struct Config {
    lua: Lua,
    pub settings: Settings,
}

const API: &str = r#"
fvim = { _options = {}, cursor = {}, themes = {}, highlights = {} }
local options = setmetatable({}, {
    __index = function(_, key) return fvim._options[key] end,
    __newindex = function(_, key, value)
        fvim._options[key] = value
        if key == 'background' and vim.g.colors_name then
            vim.cmd.colorscheme(vim.g.colors_name)
        end
    end,
    __pairs = function() return next, fvim._options, nil end,
})
fvim.opt = options
vim = { opt = fvim.opt, o = fvim.opt, bo = fvim.opt, g = {}, api = {}, cmd = {} }
local original_fvim, original_vim = fvim, vim
local function copy(t, seen)
    seen = seen or {}
    if seen[t] then return seen[t] end
    local out = {}
    seen[t] = out
    for k,v in pairs(t) do out[k] = type(v) == 'table' and copy(v, seen) or v end
    return out
end
function vim.cmd.colorscheme(name)
    local theme = assert(fvim.themes[name], 'Unknown colorscheme: '..tostring(name))
    local palette = assert(theme[vim.opt.background], 'Unknown background')
    fvim.highlights = copy(palette)
    vim.g.colors_name = name
end
function vim.api.nvim_set_hl(ns, group, values)
    assert(ns == 0, 'Only global highlights are supported')
    fvim.highlights[group] = copy(values)
end
function fvim._snapshot()
    return copy({ opt=fvim._options, cursor=fvim.cursor, themes=fvim.themes,
        highlights=fvim.highlights, output_buffer_size=fvim.output_buffer_size, g=vim.g })
end
function fvim._restore(s)
    fvim, vim = original_fvim, original_vim
    fvim._options = s.opt
    fvim.opt, vim.opt, vim.o, vim.bo = options, options, options, options
    fvim.cursor, fvim.themes, fvim.highlights = s.cursor, s.themes, s.highlights
    fvim.output_buffer_size, vim.g = s.output_buffer_size, s.g
end
"#;

fn exec(lua: &Lua, text: &str, name: &str) -> Result<(), String> {
    lua.load(text)
        .set_name(name)
        .exec()
        .map_err(|e| format!("{name}: {e}"))
}

impl Config {
    pub fn execute(&mut self, script: &str, name: &str) -> Result<(), String> {
        let f: Table = self.lua.globals().get("fvim").map_err(|e| e.to_string())?;
        let snapshot: mlua::Function = f.get("_snapshot").map_err(|e| e.to_string())?;
        let restore: mlua::Function = f.get("_restore").map_err(|e| e.to_string())?;
        let saved: Table = snapshot.call(()).map_err(|e| e.to_string())?;
        let result = exec(&self.lua, script, name)
            .and_then(|()| read_settings(&self.lua).map_err(|e| format!("{name}: {e}")));
        match result {
            Ok(settings) => {
                self.settings = settings;
                Ok(())
            }
            Err(error) => {
                restore.call::<()>(saved).map_err(|e| e.to_string())?;
                Err(error)
            }
        }
    }

    pub fn command(&mut self, text: &str, dir: &Path) -> Result<(), String> {
        let (name, argument) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
        let argument = argument.trim();
        match name {
            "lua" => self.execute(argument, ":lua"),
            "colorscheme" => {
                let literal = format!("{:?}", argument);
                self.execute(&format!("vim.cmd.colorscheme({literal})"), ":colorscheme")
            }
            "source" if argument.is_empty() => {
                *self = Self::load(dir)?;
                Ok(())
            }
            "source" | "luafile" => {
                let path = if let Some(rest) = argument.strip_prefix("~/") {
                    std::env::var_os("HOME")
                        .map(std::path::PathBuf::from)
                        .ok_or("HOME is missing")?
                        .join(rest)
                } else {
                    argument.into()
                };
                let script =
                    fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                self.execute(&script, &path.to_string_lossy())
            }
            "set" => {
                if argument.is_empty() {
                    return Err("Use :set option=value, :set option or :set nooption".into());
                }
                let mut script = String::new();
                for token in argument.split_whitespace() {
                    let (key, value) = if let Some((k, v)) = token.split_once('=') {
                        (
                            k,
                            if let Ok(n) = v.parse::<i64>() {
                                n.to_string()
                            } else {
                                format!("{v:?}")
                            },
                        )
                    } else if let Some(key) = token.strip_prefix("no") {
                        (key, "false".into())
                    } else {
                        (token, "true".into())
                    };
                    let key = match key {
                        "nu" => "number",
                        "rnu" => "relativenumber",
                        "ts" => "tabstop",
                        "sw" => "shiftwidth",
                        "et" => "expandtab",
                        "si" => "smartindent",
                        "so" => "scrolloff",
                        "siso" => "sidescrolloff",
                        "sidescroll" => "sidescroll",
                        "ls" => "laststatus",
                        "ch" => "cmdheight",
                        "stl" => "statusline",
                        other => other,
                    };
                    script.push_str(&format!("vim.opt[{key:?}]={value};"));
                }
                self.execute(&script, ":set")
            }
            _ => Err(format!("Unknown configuration command: {name}")),
        }
    }
    pub fn from_scripts(defaults: &str, init: &str) -> Result<Self, String> {
        let lua = Lua::new();
        exec(&lua, API, "fvim API")?;
        exec(&lua, defaults, "pre_configured.lua")?;
        read_settings(&lua).map_err(|e| format!("pre_configured.lua: {e}"))?;
        exec(&lua, init, "init.lua")?;
        let settings = read_settings(&lua).map_err(|e| format!("init.lua: {e}"))?;
        Ok(Self { lua, settings })
    }

    pub fn load(dir: &Path) -> Result<Self, String> {
        fn read(path: &Path) -> Result<Option<String>, String> {
            match fs::read_to_string(path) {
                Ok(text) => Ok(Some(text)),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(format!("{}: {e}", path.display())),
            }
        }
        let defaults = read(&dir.join("pre_configured.lua"))?;
        let init = read(&dir.join("init.lua"))?;
        Self::from_scripts(
            defaults.as_deref().unwrap_or(DEFAULTS),
            init.as_deref().unwrap_or(""),
        )
        .map_err(|e| format!("{}: {e}", dir.display()))
    }
}

fn read_settings(lua: &Lua) -> Result<Settings, String> {
    fn get<T: mlua::FromLua>(table: &Table, key: &str) -> Result<T, String> {
        table.get(key).map_err(|e| format!("{key}: {e}"))
    }
    fn number(table: &Table, key: &str, min: usize, max: usize) -> Result<usize, String> {
        let value = match get::<Value>(table, key)? {
            Value::Integer(n) => n,
            Value::Number(n)
                if n.is_finite() && n.fract() == 0.0 && n >= min as f64 && n <= max as f64 =>
            {
                n as i64
            }
            _ => return Err(format!("{key} must be an integer")),
        };
        if value < min as i64 || value > max as i64 {
            Err(format!("{key} must be between {min} and {max}"))
        } else {
            Ok(value as usize)
        }
    }
    let f: Table = get(&lua.globals(), "fvim")?;
    let opt: Table = get(&f, "_options")?;
    let known = [
        "number",
        "relativenumber",
        "numberwidth",
        "signcolumn",
        "tabstop",
        "shiftwidth",
        "expandtab",
        "smartindent",
        "termguicolors",
        "wrap",
        "linebreak",
        "scrolloff",
        "sidescroll",
        "sidescrolloff",
        "laststatus",
        "cmdheight",
        "showmode",
        "statusline",
        "fillchars",
        "background",
    ];
    for pair in opt.clone().pairs::<String, Value>() {
        let (key, _) = pair.map_err(|e| e.to_string())?;
        if !known.contains(&key.as_str()) {
            return Err(format!("Unsupported option: {key}"));
        }
    }
    for key in [
        "number",
        "relativenumber",
        "expandtab",
        "smartindent",
        "termguicolors",
        "wrap",
        "linebreak",
        "showmode",
    ] {
        if !matches!(get::<Value>(&opt, key)?, Value::Boolean(_)) {
            return Err(format!("{key} must be a boolean"));
        }
    }
    let background: String = get(&opt, "background")?;
    if !matches!(background.as_str(), "dark" | "light") {
        return Err("background must be dark or light".into());
    }
    let sign: String = get(&opt, "signcolumn")?;
    if !matches!(sign.as_str(), "yes" | "no" | "auto") {
        return Err("signcolumn must be yes, no or auto".into());
    }
    let fill: Table = get(&opt, "fillchars")?;
    let eob: String = get(&fill, "eob")?;
    if !eob.is_empty()
        && (eob.chars().count() != 1
            || unicode_width::UnicodeWidthStr::width(eob.as_str()) != 1
            || eob.chars().any(char::is_control))
    {
        return Err("fillchars.eob must be empty or one terminal cell".into());
    }
    let cursor: Table = get(&f, "cursor")?;
    let normal_cursor: String = get(&cursor, "normal")?;
    let insert_cursor: String = get(&cursor, "insert")?;
    for shape in [&normal_cursor, &insert_cursor] {
        if !matches!(shape.as_str(), "block" | "bar" | "underline") {
            return Err("cursor shape must be block, bar or underline".into());
        }
    }
    let statusline: String = get(&opt, "statusline")?;
    let mut format = statusline.chars();
    while let Some(ch) = format.next() {
        if ch == '%' {
            let token = format.next().ok_or("Incomplete statusline format")?;
            if !"fFtMm lcvLpP%=".replace(' ', "").contains(token) {
                return Err(format!("Unsupported statusline token: %{token}"));
            }
        }
    }
    let hl: Table = get(&f, "highlights")?;
    let mut highlights = HashMap::new();
    for pair in hl.clone().pairs::<String, Table>() {
        let (name, _) = pair.map_err(|e| e.to_string())?;
        let value = read_highlight(&hl, &name, &mut Vec::new())
            .map_err(|e| format!("highlight {name}: {e}"))?;
        highlights.insert(name, value);
    }
    Ok(Settings {
        number: get(&opt, "number")?,
        relativenumber: get(&opt, "relativenumber")?,
        numberwidth: number(&opt, "numberwidth", 1, 20)?,
        signcolumn: sign == "yes",
        tabstop: number(&opt, "tabstop", 1, 256)?,
        shiftwidth: number(&opt, "shiftwidth", 0, 256)?,
        expandtab: get(&opt, "expandtab")?,
        smartindent: get(&opt, "smartindent")?,
        termguicolors: get(&opt, "termguicolors")?,
        wrap: get(&opt, "wrap")?,
        linebreak: get(&opt, "linebreak")?,
        scrolloff: number(&opt, "scrolloff", 0, 1_000_000)?,
        sidescroll: number(&opt, "sidescroll", 0, 1_000_000)?,
        sidescrolloff: number(&opt, "sidescrolloff", 0, 1_000_000)?,
        laststatus: number(&opt, "laststatus", 0, 3)?,
        cmdheight: number(&opt, "cmdheight", 0, 20)?,
        showmode: get(&opt, "showmode")?,
        statusline,
        eob,
        normal_cursor,
        insert_cursor,
        output_buffer_size: number(&f, "output_buffer_size", 1, 1_048_576)?,
        highlights,
    })
}

fn read_highlight(groups: &Table, name: &str, path: &mut Vec<String>) -> Result<Highlight, String> {
    if path.iter().any(|p| p == name) {
        return Err("highlight link cycle".into());
    }
    let table: Table = groups.get(name).map_err(|e| e.to_string())?;
    let link: Option<String> = table.get("link").map_err(|e| e.to_string())?;
    if let Some(link) = link {
        path.push(name.into());
        let result = read_highlight(groups, &link, path);
        path.pop();
        return result;
    }
    fn rgb(table: &Table, key: &str) -> Result<Option<(u8, u8, u8)>, String> {
        let value: Option<String> = table.get(key).map_err(|e| e.to_string())?;
        let Some(value) = value else { return Ok(None) };
        if value == "NONE" {
            return Ok(None);
        }
        let value = value
            .strip_prefix('#')
            .ok_or("colors must use #rrggbb or NONE")?;
        if value.len() != 6 || !value.is_ascii() {
            return Err("colors must use #rrggbb".into());
        }
        let n = u32::from_str_radix(value, 16).map_err(|_| "Invalid RGB color")?;
        Ok(Some(((n >> 16) as u8, (n >> 8) as u8, n as u8)))
    }
    fn flag(table: &Table, key: &str) -> Result<bool, String> {
        match table.get::<Value>(key).map_err(|e| e.to_string())? {
            Value::Nil => Ok(false),
            Value::Boolean(value) => Ok(value),
            _ => Err(format!("{key} must be a boolean")),
        }
    }
    Ok(Highlight {
        fg: rgb(&table, "fg")?,
        bg: rgb(&table, "bg")?,
        ctermfg: table.get("ctermfg").map_err(|e| e.to_string())?,
        ctermbg: table.get("ctermbg").map_err(|e| e.to_string())?,
        bold: flag(&table, "bold")?,
        italic: flag(&table, "italic")?,
        underline: flag(&table, "underline")?,
        reverse: flag(&table, "reverse")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_options_are_rejected_instead_of_becoming_truthy() {
        for script in [
            "vim.opt.number = 'no'",
            "vim.opt.number = nil",
            "vim.opt.wrap = nil",
            "vim.opt.tabstop = 0",
            "vim.opt.tabstop = 2.5",
            "vim.opt.statusline = '%z'",
            "vim.opt.sidescrolloff = -1",
            "vim.opt.made_up = true",
            "vim.opt.fillchars = {eob = '界'}",
            "fvim.cursor.insert = 'unknown'",
        ] {
            assert!(
                Config::from_scripts(DEFAULTS, script).is_err(),
                "Accepted: {script}"
            );
        }
    }

    #[test]
    fn lua_theme_overrides_and_links_reach_typed_settings() {
        let c = Config::from_scripts(
            DEFAULTS,
            "\
            vim.opt.background = 'light'; vim.cmd.colorscheme('retrobox');\
            vim.api.nvim_set_hl(0, 'LineNr', {fg='#123456', bold=true});\
            vim.api.nvim_set_hl(0, 'CursorLineNr', {link='LineNr'})",
        )
        .unwrap();
        assert_eq!(c.settings.highlight("Normal").bg, Some((251, 241, 199)));
        assert_eq!(c.settings.highlight("CursorLineNr").fg, Some((18, 52, 86)));
        assert!(c.settings.highlight("CursorLineNr").bold);
        assert_eq!(c.settings.highlight("LineNr").ctermfg, None);
        let bg = Config::from_scripts(DEFAULTS, "vim.api.nvim_set_hl(0, 'Visual', {bg='#123456'})")
            .unwrap();
        assert_eq!(bg.settings.highlight("Visual").ctermbg, None);
        assert!(Config::from_scripts(
            DEFAULTS,
            "vim.api.nvim_set_hl(0, 'LineNr', {link='LineNr'})"
        )
        .is_err());
        assert!(
            Config::from_scripts(DEFAULTS, "vim.api.nvim_set_hl(0, 'LineNr', {fg='#xyzxyz'})")
                .is_err()
        );
    }

    #[test]
    fn runtime_lua_errors_keep_the_previous_options_and_highlights() {
        let mut c = Config::from_scripts(DEFAULTS, "").unwrap();
        let before = c.settings.clone();
        assert!(c
            .execute(
                "vim.opt.tabstop=8; vim.opt.wrap=true; error('broken')",
                "live.lua"
            )
            .is_err());
        assert_eq!(c.settings, before);
        c.execute("assert(vim.opt.tabstop==4); vim.opt.tabstop=2", "live.lua")
            .unwrap();
        assert_eq!(c.settings.tabstop, 2);
        assert!(c.execute("vim.opt.tabstop=0", "live.lua").is_err());
        c.execute("assert(vim.opt.tabstop==2)", "live.lua").unwrap();
    }

    #[test]
    fn set_commands_accept_number_and_apply_multiple_options_atomically() {
        let mut c = Config::from_scripts(DEFAULTS, "").unwrap();
        c.command("set number ts=8 sw=2 nowrap", Path::new("."))
            .unwrap();
        assert!(c.settings.number);
        assert_eq!(c.settings.tabstop, 8);
        assert_eq!(c.settings.shiftwidth, 2);
        assert!(!c.settings.wrap);
        let before = c.settings.clone();
        assert!(c.command("set wrap ts=0", Path::new(".")).is_err());
        assert_eq!(c.settings, before);
        c.command("set background=light", Path::new(".")).unwrap();
        assert_eq!(c.settings.highlight("Normal").bg, Some((251, 241, 199)));
    }
}
