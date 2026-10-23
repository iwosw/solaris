//! Offline-linked Java bytecode uses the ordinary Solaris component ABI.

use solaris_plugin_sdk::{
    export_plugin, log, message_session, BuildContext, BuildDecision, BuildResult, Command, Config,
    Event, EventContext, Failure, HookActor, InitContext, LogLevel, Plugin,
};
use std::cell::{Cell, RefCell};

#[link(wasm_import_module = "bukkit")]
extern "C" {
    fn enable();
    fn disable();
    fn deliver(kind: i32, session: u64, argc: i32);
    fn break_block(session: u64, previous: u32, proposed: u32, x: i32, y: i32, z: i32) -> i32;
    #[link_name = "teavm_catchException"]
    fn catch_exception() -> i32;
}

thread_local! {
    static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static INPUT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static COMMANDS: RefCell<Vec<Command>> = const { RefCell::new(Vec::new()) };
    static IN_HOOK: Cell<bool> = const { Cell::new(false) };
}

/// Java transfers bytes, never pointers into its independent linear memory.
#[no_mangle]
pub extern "C" fn output_byte(value: i32) {
    OUTPUT.with_borrow_mut(|buffer| {
        assert!(
            buffer.len() < 8192,
            "Java output exceeds the transfer budget"
        );
        buffer.push(u8::try_from(value).expect("Java output byte"));
    });
}

#[no_mangle]
pub extern "C" fn finish_output(kind: i32, session: u64) {
    assert!(
        !IN_HOOK.get(),
        "Bukkit precommit callbacks cannot emit effects"
    );
    OUTPUT.with_borrow_mut(|buffer| {
        let message = std::str::from_utf8(buffer).expect("valid Java UTF-8 output");
        match kind {
            0 => log(LogLevel::Info, message),
            1 => COMMANDS.with_borrow_mut(|commands| {
                assert!(commands.len() < 256, "Java command batch exceeds budget");
                commands.push(message_session(session, message));
            }),
            _ => panic!("invalid Java output kind"),
        }
        buffer.clear();
    });
}

#[no_mangle]
pub extern "C" fn input_length(field: usize) -> usize {
    INPUT.with_borrow(|fields| fields[field].len())
}

#[no_mangle]
pub extern "C" fn input_byte(field: usize, offset: usize) -> u32 {
    INPUT.with_borrow(|fields| u32::from(fields[field].as_bytes()[offset]))
}

fn check_exception() {
    // This consumes TeaVM's pending exception. A fault retires the component via
    // the existing host trap path; it never becomes a fabricated successful call.
    assert_eq!(unsafe { catch_exception() }, 0, "uncaught Java exception");
}

fn take_commands() -> Vec<Command> {
    COMMANDS.with_borrow_mut(std::mem::take)
}

#[derive(Default)]
struct Bukkit;

impl Plugin for Bukkit {
    fn init(&mut self, _config: &Config, _context: &InitContext) -> Result<Vec<Command>, Failure> {
        unsafe { enable() };
        check_exception();
        Ok(take_commands())
    }

    fn on_events(
        &mut self,
        _context: &EventContext,
        events: &[Event],
    ) -> Result<Vec<Command>, Failure> {
        for event in events {
            match event {
                Event::PlayerJoined(joined) => {
                    INPUT.with_borrow_mut(|fields| {
                        fields.clear();
                        fields.extend([joined.player.clone(), joined.name.clone()]);
                    });
                    unsafe { deliver(1, joined.session, 0) };
                }
                Event::CommandInvoked(command) => {
                    INPUT.with_borrow_mut(|fields| {
                        fields.clear();
                        fields.extend([
                            command.player.clone(),
                            command.username.clone(),
                            command.name.clone(),
                        ]);
                        fields.extend(command.arguments.iter().cloned());
                    });
                    unsafe {
                        deliver(
                            2,
                            command.session,
                            i32::try_from(command.arguments.len()).expect("bounded command args"),
                        )
                    };
                }
                _ => continue,
            }
            check_exception();
        }
        INPUT.with_borrow_mut(Vec::clear);
        Ok(take_commands())
    }

    fn before_build(&mut self, context: &BuildContext) -> Result<BuildResult, Failure> {
        let mut decision = BuildDecision::Keep;
        if let HookActor::Player(player) = &context.actor {
            INPUT.with_borrow_mut(|fields| {
                fields.clear();
                fields.push(player.uuid.clone());
            });
            IN_HOOK.set(true);
            for edit in &context.edits {
                let cancelled = unsafe {
                    break_block(
                        player.session,
                        edit.previous_state,
                        edit.proposed_state,
                        edit.x,
                        edit.y,
                        edit.z,
                    )
                };
                check_exception();
                if cancelled != 0 {
                    decision = BuildDecision::Cancel;
                    break;
                }
            }
            IN_HOOK.set(false);
            INPUT.with_borrow_mut(Vec::clear);
        }
        Ok(BuildResult {
            decision,
            expected_plugin_storage_versions: Vec::new(),
        })
    }

    fn shutdown(&mut self) -> Result<(), Failure> {
        unsafe { disable() };
        check_exception();
        assert!(
            take_commands().is_empty(),
            "onDisable cannot enqueue messages"
        );
        Ok(())
    }
}

export_plugin!(Bukkit);
