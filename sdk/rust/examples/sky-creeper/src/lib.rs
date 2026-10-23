//! Data-only native mob composition, spawned by /sky-creeper in the caller's dimension.
use solaris_plugin_sdk::{
    Command, Config, Event, EventContext, Failure, Plugin, StartupContribution, commands,
    export_plugin,
};

#[derive(Default)]
struct SkyCreeper;

impl Plugin for SkyCreeper {
    fn configure(&mut self, _config: &Config) -> Result<Option<StartupContribution>, Failure> {
        Ok(Some(StartupContribution {
            placement: None,
            trees: None,
            clay: None,
            spawning: None,
            items: None,
            work_recipes: None,
            crafting_recipes: None,
            block_tools: None,
            mob_blueprints: Some(vec![include_str!("sky-creeper.json").to_owned()]),
        }))
    }

    fn on_events(
        &mut self,
        _context: &EventContext,
        events: &[Event],
    ) -> Result<Vec<Command>, Failure> {
        Ok(events
            .iter()
            .filter_map(|event| {
                let Event::CommandInvoked(command) = event else {
                    return None;
                };
                (command.name == "sky-creeper").then(|| {
                    Command::SpawnEntity(commands::SpawnEntity {
                        request: "sky-creeper".into(),
                        session: command.session,
                        entity_type: "sky-creeper:sky_creeper".into(),
                        position: solaris_plugin_sdk::types::Position {
                            x: command.position.x + 8.0,
                            y: command.position.y + 6.0,
                            z: command.position.z,
                        },
                    })
                })
            })
            .collect())
    }
}

export_plugin!(SkyCreeper);
