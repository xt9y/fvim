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
            SnippetPlaceholder = { bg = '#243447', ctermbg = 24 },
            SnippetPlaceholderActive = { bg = '#36597a', ctermbg = 25, bold = true },
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
            SnippetPlaceholder = { bg = '#dbe9ee', ctermbg = 254 },
            SnippetPlaceholderActive = { bg = '#b0d0d0', ctermbg = 152, bold = true },
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
    fallback = { 'c', 'build', 'run' },
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
vim.keymap.set('n', 'pc', 'preconfig')
vim.keymap.set('n', 'gcc', 'comment')
vim.keymap.set('n', 'gbc', 'blockcomment')
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

-- Language tooling and diagnostic UI, matching xt9y/config/nvim.
vim.opt.updatetime = 300
vim.opt.autocomplete = true
vim.opt.pumheight = 5
fvim.tooling = {
    syntax = true,
    languages = { 'c', 'cpp', 'lua', 'zig', 'odin', 'bash', 'json', 'markdown', 'hlsl' },
    servers = {
        clangd = { cmd={'clangd', '--background-index'}, filetypes={'c','cpp','objc','objcpp'}, root_markers={'compile_commands.json','compile_flags.txt','.git'}, enabled=true },
        zls = { cmd={'zls'}, filetypes={'zig'}, root_markers={'build.zig','zls.json','.git'}, enabled=true },
        ols = { cmd={'ols'}, filetypes={'odin'}, root_markers={'ols.json','.git'}, enabled=true },
        slangd = { cmd={'slangd'}, filetypes={'hlsl'}, root_markers={'slangd.json','.git'}, enabled=true },
    },
    diagnostics = {},
}
vim.diagnostic.config {
    virtual_text={spacing=4, prefix='●'}, signs=true, underline=true,
    update_in_insert=false, severity_sort=true, float={border='rounded',source='always'},
}
vim.filetype.add { extension={fx='hlsl',fxh='hlsl',usf='hlsl',ush='hlsl',json='json',md='markdown',sh='bash'} }
vim.keymap.set('n','<leader>d','diagnostics')
vim.keymap.set('n',']d','diagnostic_next')
vim.keymap.set('n','[d','diagnostic_previous')
vim.keymap.set('n','<leader>e','diagnostic_float')
vim.keymap.set('n','K','hover')
vim.keymap.set('n','gd','definition')
vim.keymap.set('n','gr','references')
local function native_highlight(group, values)
    fvim.themes.retrobox.dark[group] = values
    fvim.themes.retrobox.light[group] = values
    vim.api.nvim_set_hl(0, group, values)
end
native_highlight('DiagnosticError',{fg='#fb4934',ctermfg=167})
native_highlight('DiagnosticWarn',{fg='#fabd2f',ctermfg=214})
native_highlight('DiagnosticInfo',{fg='#83a598',ctermfg=109})
native_highlight('DiagnosticHint',{fg='#8ec07c',ctermfg=108})
native_highlight('Comment',{fg='#928374',ctermfg=102,italic=true})
native_highlight('String',{fg='#b8bb26',ctermfg=142})
native_highlight('Number',{fg='#d3869b',ctermfg=175})
native_highlight('Keyword',{fg='#fb4934',ctermfg=167,bold=true})
native_highlight('Type',{fg='#fabd2f',ctermfg=214})
native_highlight('Function',{fg='#8ec07c',ctermfg=108})
native_highlight('Variable',{fg='#83a598',ctermfg=109})
native_highlight('Property',{fg='#fe8019',ctermfg=208})
native_highlight('Constant',{fg='#d3869b',ctermfg=175})

fvim.comments.bash = fvim.comments.sh
