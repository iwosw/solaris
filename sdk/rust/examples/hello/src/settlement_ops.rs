//! Native settlement S1–S4 acceptance fixture: one closed mutation at a time.
//!
//! `mode = "settlement-operations"` runs this module. A player runs the
//! package's `settlement-api` root with exactly one action as its arguments,
//! and the fixture answers exactly one native settlement operation of the
//! contract: a query, a bind, an unbind, a rename, a household form/relocate,
//! a district plan, or a typed economic quote/accept against the settlement id
//! the query observed, fenced by the revision it carried. Like the npc fixture,
//! this only builds the
//! operation envelope; the server's own DTOs validate and the native
//! settlement owner decides.

use solaris_plugin_sdk::events::{CommandInvoked, Event};
use solaris_plugin_sdk::{
    bind_settlement, form_settlement_household, log, plan_settlement_district, query_settlements,
    relocate_settlement_household, rename_settlement, unbind_settlement, Command, Config, Failure,
    LogLevel,
};

/// The command root the package's manifest declares for this fixture.
pub(crate) const ROOT: &str = "settlement-api";

/// The configuration `mode` that selects this fixture.
const MODE: &str = "settlement-operations";

/// The bounded reference consumer: one settlement operation per command, no
/// governance and no state carried between the startup phases.
pub struct Fixture;

impl Fixture {
    pub fn configure(config: &Config) -> Result<Self, Failure> {
        let toml = config.toml();
        let mode = toml
            .as_ref()
            .and_then(|value| value.get("mode"))
            .and_then(|value| value.as_str());
        if mode != Some(MODE) {
            return Err(Failure::Invalid);
        }
        Ok(Self)
    }

    pub fn init(&mut self) -> Result<Vec<Command>, Failure> {
        log(LogLevel::Info, "SETTLEMENT_API ready");
        Ok(Vec::new())
    }

    pub fn on_events(&mut self, events: &[Event]) -> Result<Vec<Command>, Failure> {
        let mut commands = Vec::new();
        for event in events {
            let Event::CommandInvoked(invoked) = event else {
                continue;
            };
            if invoked.name != ROOT {
                continue;
            }
            if let Some(command) = Self::operation(invoked) {
                commands.push(command);
            }
        }
        Ok(commands)
    }

    fn operation(invoked: &CommandInvoked) -> Option<Command> {
        let args = &invoked.arguments;
        let argument = |index: usize| args.get(index).map(String::as_str);
        match argument(0)? {
            "query" if args.len() == 3 => Some(query_settlements(
                argument(1)?,
                &[argument(2)?.to_owned()],
                None,
            )),
            "bind" if args.len() == 6 => Some(bind_settlement(
                argument(1)?,
                argument(2)?,
                argument(3)?.parse().ok()?,
                argument(4)?,
                argument(5)?.parse().ok()?,
            )),
            "unbind" if args.len() == 6 => Some(unbind_settlement(
                argument(1)?,
                argument(2)?,
                argument(3)?.parse().ok()?,
                argument(4)?,
                argument(5)?.parse().ok()?,
            )),
            "rename" if args.len() == 6 => Some(rename_settlement(
                argument(1)?,
                argument(2)?,
                argument(3)?.parse().ok()?,
                argument(4)?,
                argument(5)?.parse().ok()?,
            )),
            "form-household" if args.len() == 6 => Some(form_settlement_household(
                argument(1)?,
                argument(2)?,
                argument(3)?.parse().ok()?,
                &argument(4)?
                    .split(',')
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
                argument(5)?.parse().ok()?,
            )),
            "relocate-household" if args.len() == 8 => Some(relocate_settlement_household(
                argument(1)?,
                argument(2)?,
                argument(3)?.parse().ok()?,
                argument(4)?,
                argument(5)?.parse().ok()?,
                argument(6)?.parse().ok()?,
                argument(7)?.parse().ok()?,
            )),
            "plan-district" if args.len() == 8 => {
                let refs = argument(7)?
                    .split(',')
                    .filter(|part| !part.is_empty())
                    .map(|part| {
                        let mut columns = part.split('|');
                        let (kind, id, purpose) =
                            (columns.next()?, columns.next()?, columns.next()?);
                        if columns.next().is_some() {
                            return None;
                        }
                        Some(solaris_plugin_sdk::settlements::DistrictBuildingRef {
                            kind: kind.to_owned(),
                            id: id.to_owned(),
                            purpose: purpose.to_owned(),
                        })
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(plan_settlement_district(
                    argument(1)?,
                    argument(2)?,
                    argument(3)?.parse().ok()?,
                    argument(4)?.parse().ok()?,
                    argument(5)?,
                    argument(6)?.parse().ok()?,
                    &refs,
                ))
            }
            "economy-quote" if args.len() == 11 => {
                Some(solaris_plugin_sdk::request_settlement_economy(
                    argument(1)?,
                    argument(2)?,
                    argument(3)?.parse().ok()?,
                    argument(4)?.parse().ok()?,
                    solaris_plugin_sdk::settlements::EconomyAction::Quote(
                        solaris_plugin_sdk::settlements::Quote {
                            id: argument(5)?.to_owned(),
                            seller:
                                solaris_plugin_sdk::settlements::EconomicInventoryEndpoint::Carry(
                                    argument(6)?.to_owned(),
                                ),
                            buyer:
                                solaris_plugin_sdk::settlements::EconomicInventoryEndpoint::Carry(
                                    argument(7)?.to_owned(),
                                ),
                            source_slot: argument(8)?.parse().ok()?,
                            quantity: argument(9)?.parse().ok()?,
                            price: argument(10)?.parse().ok()?,
                        },
                    ),
                ))
            }
            "economy-accept" if args.len() == 6 => {
                Some(solaris_plugin_sdk::request_settlement_economy(
                    argument(1)?,
                    argument(2)?,
                    argument(3)?.parse().ok()?,
                    argument(4)?.parse().ok()?,
                    solaris_plugin_sdk::settlements::EconomyAction::AcceptQuote(
                        argument(5)?.to_owned(),
                    ),
                ))
            }
            _ => None,
        }
    }
}
