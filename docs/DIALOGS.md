# Server-driven dialogs

Vanilla 26.1.2 clients (and the Solaris native client) render server-sent dialogs with no mod. The
tick decides what to show and acts on the answer; the edge only converts between packets and
core values.

| Direction | Packet | Core value |
|---|---|---|
| server → client | Play `show_dialog` (0x8C, inline holder id 0 + network NBT) | `PlayerChange::ShowDialog { dialog: Tag }` |
| server → client | Play `clear_dialog` (0x8B, empty) | `PlayerChange::ClearDialog` |
| client → server | Play `custom_click_action` (0x44, identifier + length-prefixed optional NBT) | `PlayerInput::DialogClick { connection, id, payload }` |

IDs and layouts come from the local `data/vanilla/reports/packets.json` and the decompiled 26.1.2
`ClientboundShowDialogPacket`, `ClientboundClearDialogPacket`, `ServerboundCustomClickActionPacket`
and `net.minecraft.server.dialog.*`. Registry dialog references (holder id > 0) are never sent and
are rejected on decode.

## Core API (`mc_core::dialog`)

```rust
let dialog = Dialog::multi_action("Found settlement", vec![
        Button::custom("Found", "solaris:settlement/found").with("site", Tag::Int(7)),
    ])
    .text("Pick a name and radius")
    .input(Input::text("name", "Name").max_length(16))
    .input(Input::number_range("radius", "Radius", 1.0, 32.0).initial_number(8.0).step(1.0))
    .input(Input::boolean("walls", "Walls"))
    .exit(Button::plain("Close"));
dialog::open(state, connection, dialog)?;   // queues PlayerChange::ShowDialog
dialog::clear(state, connection);           // queues PlayerChange::ClearDialog
```

`Dialog::notice`/`confirmation`/`multi_action` build the three supported dialog types; inputs are
`text`, `number_range`, `boolean`, `single_option`. `into_tag()` validates (names, ids, ranges,
duplicates) because a malformed dialog disconnects a vanilla client. Defaults: closable with
Escape, `pause = false`, `after_action = close` (`AfterAction::{None, WaitForResponse}` keep the
dialog / show "Waiting for Server" until the next `open`/`clear`).

Every button uses vanilla's `dynamic/custom` action: the click payload is a compound with the
button's `.with(..)` additions plus one entry per input, typed exactly as vanilla sends them
(text and single_option → String, boolean → Byte, number_range → Float). Read it with
`ClickPayload(payload).string/bool/float(key)`.

## Where a click arrives

`PlayerInput::DialogClick` is applied in the native tick (`tick::apply_inputs`, behind the
session-lock fence) and routed by `dialog::route_click` on the owner segment of the action id
`solaris:<owner>/<action>`. Add one arm there per owner (for example `settlement`). Unknown ids
are ignored. Clients can send any id/payload at any time: re-validate permission, range and state
in the tick before acting.

Registered owners: `dialog_probe` (test hook) and `settlement` (`settlements::ui`: `/settlement` and
`/settlement menu [page]`; every click re-resolves the settlement from the `s` payload entry and
re-checks owner/operator before mutating). Keep forms compact: a vanilla dialog scrolls, but a small
window at GUI scale 3 shows only ~240 GUI pixels, so put the buttons inside that height.

Wasm plugins do not receive dialog clicks yet (that needs a `solaris:plugin` interface change).

## Test hook and proof

`SOLARIS_DIALOG_PROBE=1` shows every joining player a multi_action dialog (text, slider,
checkbox) and logs/echoes the click payload (`solaris:dialog_probe/*`); inert otherwise.
`python3 -m tools.harness run dialogs --client native --windowed` drives it through the native
client (`SOLARIS_SERVER_BINARY` required). Unit/integration coverage with a fake connection lives in
`mc-protocol` (`dialog_tests.rs`), `mc-core` (`dialog_tests.rs`) and `mc-edge` (`dialog_tests.rs`).
