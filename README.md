# strata

A keyboard-driven PDF/EPUB reader for Wayland (written for Hyprland), in the style of zathura, built for studying
textbooks: generated index for scanned books, OCR, hover previews of equations and theorems, bookmarks, SyncTeX.
Small and quiet: no polling, no idle wakeups, capped caches, a dedicated render thread.

## Use

    strata book.pdf
    strata --exec 'goto 120'          # drive a running instance over D-Bus (--pid N to pick one)

Press `F1` for every key and command, `:` for the command line (`:set`, `:map`, `:open`, `:bmark`, ...).
Quick tour: `j k` scroll, `Space`/`J` next page, `gg G` first/last, `Tab` index, `f` follow link, `p` preview
hints, `gp` list of equations/theorems/figures, `/` search, `m a` set mark, `' a` jump to it (`A`-`Z` are global
and reopen their file), `:bmark NAME`, `gb`/`:blist` bookmarks, `:recent` recent files.

State (last position, zoom, recolor, bookmarks, marks, jumplist) is stored in `~/.local/share/strata/strata.db` and
written on close, focus loss, `:open` and SIGTERM/SIGINT; `set restore-position false` turns restoring off.

## Config

`~/.config/strata/stratarc`; see `stratarc.example` (installed to `/usr/share/doc/strata/`). All options are listed
by `:set` and in `F1`.

## OCR

Scanned books are OCRed in a background process with `ocrmypdf`:

    pipx install ocrmypdf            # or: pacman -S ocrmypdf

## D-Bus

Each instance owns `org.strata.PID-<pid>` at `/org/strata` (interface `org.strata`): methods `ExecuteCommand(s)`,
`GotoPage(u)`, `SynctexView(s,u,u)`, `OpenDocument(s)`; read-only properties `filename`, `pagenumber`,
`numberofpages`. `strata --exec CMD` targets the focused Hyprland window if it is a strata, else the newest instance.
`contrib/textbook-toggle.sh` is an example.

## SyncTeX with vimtex

Needs the `synctex` binary (TeX Live) and a document built with `-synctex=1` (latexmk does this in vimtex's default).

    let g:vimtex_view_method = 'general'
    let g:vimtex_view_general_viewer = 'strata'
    let g:vimtex_view_general_options = '--synctex-forward @line:@col:@tex @pdf'

Forward search (`\lv`) jumps to the position and highlights it until the next key press; a running strata showing
the same PDF is reused. For inverse search, `Ctrl+click` on the page runs `synctex edit` and then
`synctex-editor-command`, for example in `stratarc`:

    set synctex-editor-command "nvim --server /tmp/vimtexserver.pipe --remote-expr \"vimtex#view#inverse_search(%{line}, '%{input}')\""

(start nvim with `--listen /tmp/vimtexserver.pipe`, or use `v:servername`; `%{input}`, `%{line}`, `%{column}` are substituted.)

## Build and package

    cargo build --release            # needs libsqlite3, libdbus-1, clang
    cd packaging && makepkg -f       # strata-git package from this checkout
