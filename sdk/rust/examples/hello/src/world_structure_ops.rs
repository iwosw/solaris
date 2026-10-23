//! A generic feature/worksite query example; gameplay acceptance belongs to plugins.
use solaris_plugin_sdk::events::Event;
use solaris_plugin_sdk::{
    Command, Config, Failure, LogLevel, check_worksite, list_features, log, query_feature,
    worksite_status,
};

pub(crate) const ROOT: &str = "world-structure";
pub struct Fixture;
impl Fixture {
    pub fn configure(_config: &Config) -> Result<Self, Failure> {
        Ok(Self)
    }
    pub fn init(&mut self) -> Result<Vec<Command>, Failure> {
        Ok(vec![list_features("world-feature-list", None, 64)])
    }
    pub fn on_events(&mut self, events: &[Event]) -> Result<Vec<Command>, Failure> {
        let mut commands = Vec::new();
        for event in events {
            match event {
                Event::CommandInvoked(v) if v.name == ROOT => match v.arguments.as_slice() {
                    [name] if name == "list" => {
                        commands.push(list_features("world-feature-list", None, 64))
                    }
                    [name, id] if name == "feature" => {
                        commands.push(query_feature("world-feature-query", id))
                    }
                    [name, id] if name == "worksite" => {
                        commands.push(worksite_status("world-worksite-status", id))
                    }
                    [name, id] if name == "check" => {
                        commands.push(check_worksite("world-worksite-check", id))
                    }
                    _ => log(
                        LogLevel::Error,
                        "world-structure expects list | feature ID | worksite ID | check ID",
                    ),
                },
                Event::OperationAnswered(v) if v.request.starts_with("world-") => {
                    log(LogLevel::Info, &format!("WORLD_STRUCTURE {:?}", v.outcome))
                }
                _ => {}
            }
        }
        Ok(commands)
    }
}
