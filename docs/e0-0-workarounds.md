# E0-0 nvim-oxi API Spike Workarounds

Spike validations (crates/oxi-spike + tests crate) confirmed the following:

## APIs Validated

1. **Timer (libuv)** - `TimerHandle::start` with callback fires on interval. `timer.stop()` works.

2. **Buffer-local keymap with callback** - `buf.set_keymap` with `SetKeymapOpts::builder().callback(|| {...})` and rhs `""` invokes the Rust closure when the key is pressed.

3. **User command with nargs** - `api::create_user_command` with `CommandNargs::Any` passes `CommandArgs` to the callback; `args.args()` yields the arg strings.

4. **schedule from background thread** - `std::thread::spawn` + `nvim_oxi::schedule(move |_| {...})` runs the closure on the main thread. Must not call Neovim API from the worker thread.

5. **Extmarks with sign_text/virt_text** - `SetExtmarkOpts::builder().sign_text(...).sign_hl_group(...).virt_text(...).virt_text_pos(...)` produces visible sign column and overlay text.

6. **Floating window** - `api::open_win` with `OpenWinOpts::builder().relative(Editor).width(...).height(...).row(...).col(...).border(...).title(...)` creates a bordered float.

7. **#[nvim_oxi::test]** - Tests crate cdylib with `nvim_oxi::tests::build()` in build.rs; `#[nvim_oxi::test]` spawns Neovim and runs the test.

## Workarounds

- **Buffer::set_keymap** - Requires `&mut self`. Use `let mut buf = ...` and pass `&mut buf` or ensure the buffer binding is mutable.
- **IntoResult** - Many API calls return `Result`-like types. Use `.into_result()?` when the return type is not `Result` directly.
- **schedule callback signature** - Callback receives one argument (often `()`). Use `move |_|` when ignoring it.
- **virt_text** - Pass an array of `(text, hl_group)` tuples, e.g. `[("text", "Comment")]`.

## No Blockers

All seven touchpoints work as designed. No architecture changes required.
