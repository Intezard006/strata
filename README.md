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

`Ctrl-p` opens a command palette over commands, index entries, bookmarks, previews and recent files (prefix `>`
commands, `#` index, `@` bookmarks, `=` previews, `%` recent). `:goto 120` goes to printed page 120 when the book has
page numbers, `:goto #120` to PDF page 120, `:goto xii` to front matter. `Ctrl-t` cycles look profiles (`normal`,
`textbook`, or your own `profile NAME OPTION VALUE` lines). `gf` opens a small floating, pinned window with the current
page (or the preview target) inside the same process; `strata --float FILE --page N` does the same from a script.
Mouse: `wheel-step` (px per notch), `touchpad-scroll-factor`, `wheel-zoom-step` (Ctrl+wheel; 0 = `zoom-step`).

Images keep their colours in recolor mode (`set recolor-images keep|dim|recolor`).
In the default `smart` mode each image is classified once (line art is recolored, photos keep their colours) and the
decision is stored per document in `~/.cache/strata/<key>/images.bin`; a background pass fills in the rest. `gi`
(`:image-toggle`) cycles the image under the cursor, or the largest one on screen, through keep / recolor / auto; the
override is stored in the same file and survives reclassification.

With `auto-reload` (default on) the file is reloaded in place when it changes on disk (a `latexmk` build), keeping
page, offset and zoom; a file that does not open yet (half written) keeps the old view. `:reload` does it by hand.

`d` (`:book`) toggles book mode: two-page spreads with the cover alone (`first-page-single`, `book-rtl`); `J`/`K` move by
spread and fit-width / fit-page fit the spread. Links, search hits, previews and SyncTeX all go through the same
page-to-screen transform, so they stay correct.

`F5` (`:present`) is a presentation: fullscreen, the current page fitted to the screen, no statusbar, only the next
page prefetched. `j k Space BackSpace` the arrows and a left click move one page; `Esc q F5` end it and restore the
previous page, offset and zoom exactly. `presentation-bg` sets the surround; recolor and profiles apply as usual.

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
