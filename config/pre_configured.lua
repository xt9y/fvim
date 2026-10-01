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

-- Native workflows. init.lua can replace or delete these mappings.
fvim.workflow = {
    timeout_ms = 500,
    make = { 'make' },
    exclude = { '.git', 'target', 'node_modules', '.cache', 'build' },
    max_files = 20000,
    max_bytes = 1048576,
}
vim.keymap.set('n', '<leader><leader>', 'files')
vim.keymap.set('n', '<leader><CR>', 'grep')
vim.keymap.set('n', 'hh', 'vsplit')
vim.keymap.set('n', 'vv', 'split')
vim.keymap.set('n', 'mm', 'make')
vim.keymap.set('n', 'cc', 'config')
vim.keymap.set('n', 'gcc', 'comment')
vim.keymap.set('n', 'gbc', 'blockcomment')
vim.keymap.set('v', 'gcc', 'comment')
vim.keymap.set('v', 'gbc', 'blockcomment')
vim.filetype.add { extension = {
    c='c', h='c', cpp='cpp', hpp='cpp', cc='cpp', rs='rust',
    lua='lua', py='python', sh='sh', ll='llvm', llvm='llvm',
    zig='zig', odin='odin', hlsl='hlsl', hlsli='hlsl',
} }
fvim.comments = {
    c={line='//', open='/*', close='*/'},
    cpp={line='//', open='/*', close='*/'},
    rust={line='//', open='/*', close='*/'},
    zig={line='//'}, odin={line='//', open='/*', close='*/'},
    hlsl={line='//', open='/*', close='*/'}, llvm={line=';'},
    lua={line='--', open='--[[', close=']]'},
    python={line='#'}, sh={line='#'},
}
fvim.filetype_options = {
    llvm={tabstop=2, shiftwidth=2, expandtab=true},
    zig={tabstop=4, shiftwidth=4, expandtab=true},
    odin={tabstop=4, shiftwidth=4, expandtab=false},
    hlsl={tabstop=4, shiftwidth=4, expandtab=true},
}
