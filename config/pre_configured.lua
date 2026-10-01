-- fvim defaults. Loaded before init.lua; put personal overrides in init.lua.
vim.opt.number = true
vim.opt.relativenumber = true
vim.opt.numberwidth = 4
vim.opt.signcolumn = 'yes'
vim.opt.tabstop = 4
vim.opt.shiftwidth = 4
vim.opt.expandtab = true
vim.opt.smartindent = true
vim.opt.termguicolors = true
vim.opt.wrap = false
vim.opt.linebreak = false
vim.opt.scrolloff = 900
vim.opt.sidescroll = 1
vim.opt.sidescrolloff = 10
vim.opt.laststatus = 2
vim.opt.cmdheight = 1
vim.opt.showmode = true
vim.opt.statusline = '%f %m%=%l,%c %p%%'
vim.opt.fillchars = { eob = '~' }
fvim.cursor = { normal = 'block', insert = 'bar' }
fvim.output_buffer_size = 8192
vim.g.mapleader = ' '
vim.g.maplocalleader = ' '
vim.opt.background = 'dark'

-- Retrobox UI palette: Maxim Kim, Lifepillar and morhetz.
-- https://github.com/vim/colorschemes (retrobox); colors are configurable here.
fvim.themes = {
    retrobox = {
        dark = {
            Normal = { fg = '#ebdbb2', bg = '#1c1c1c', ctermfg = 187, ctermbg = 234 },
            LineNr = { fg = '#7c6f64', ctermfg = 243 },
            CursorLineNr = { fg = '#fabd2f', ctermfg = 214, bold = true },
            SignColumn = { fg = '#928374', ctermfg = 102 },
            EndOfBuffer = { fg = '#504945', ctermfg = 239 },
            StatusLine = { fg = '#ebdbb2', bg = '#504945', ctermfg = 187, ctermbg = 239, bold = true },
            Visual = { bg = '#2a405a', ctermbg = 24 },
            ModeMsg = { fg = '#fabd2f', ctermfg = 214, bold = true },
        },
        light = {
            Normal = { fg = '#3c3836', bg = '#fbf1c7', ctermfg = 237, ctermbg = 230 },
            LineNr = { fg = '#a89984', ctermfg = 137 },
            CursorLineNr = { fg = '#b57614', ctermfg = 172, bold = true },
            SignColumn = { fg = '#3c3836', ctermfg = 237 },
            EndOfBuffer = { fg = '#e5d4b1', ctermfg = 187 },
            StatusLine = { fg = '#3c3836', bg = '#bdae93', ctermfg = 237, ctermbg = 144, bold = true },
            Visual = { bg = '#b0d0d0', ctermbg = 152 },
            ModeMsg = { fg = '#3c3836', ctermfg = 237, bold = true },
        },
    },
    default = { dark = {}, light = {} },
}
vim.cmd.colorscheme('retrobox')
