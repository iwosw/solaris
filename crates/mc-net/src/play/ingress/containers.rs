use super::super::*;
use crate::play::containers::handle_client_close;

pub(crate) struct ContainerIngressContext<'a, W> {
    pub(in crate::play) writer: &'a mut W,
    pub(in crate::play) interaction: Option<&'a mut InteractionState>,
    pub(in crate::play) script_gameplay_events: Option<&'a ScriptGameplayEventPublisher>,
    pub(in crate::play) scripts: Option<&'a ScriptEventSink>,
    pub(in crate::play) session_id: SessionId,
    pub(in crate::play) player_uuid: &'a str,
    pub(in crate::play) player_name: &'a str,
    pub(in crate::play) permissions: CommandPermissions,
    pub(in crate::play) game_mode: GameMode,
    pub(in crate::play) survival_state: &'a mut SurvivalState,
    pub(in crate::play) xp_state: &'a mut XpState,
    pub(in crate::play) player_pose: PlayerPose,
}

pub(crate) fn is_serverbound_container_packet(id: i32) -> bool {
    id == ServerboundPlaceRecipe::ID
        || id == ServerboundSelectTrade::ID
        || id == ServerboundContainerButtonClick::ID
        || id == ServerboundContainerClick::ID
        || id == ServerboundContainerClose::ID
        || id == ServerboundSetCreativeModeSlot::ID
}

pub(crate) async fn handle_serverbound_container<W>(
    context: ContainerIngressContext<'_, W>,
    frame: mc_protocol::RawFrame,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let ContainerIngressContext {
        writer,
        mut interaction,
        script_gameplay_events,
        scripts,
        session_id,
        player_uuid,
        player_name,
        permissions,
        game_mode,
        survival_state,
        xp_state,
        player_pose,
    } = context;
    let mut body = frame.body;
    match frame.id {
        ServerboundPlaceRecipe::ID => {
            let recipe = ServerboundPlaceRecipe::decode(&mut body)?;
            if let Some(state) = interaction.as_deref_mut() {
                handle_place_recipe(
                    state,
                    writer,
                    script_gameplay_events,
                    player_pose,
                    game_mode,
                    *survival_state,
                    recipe,
                )
                .await?;
            } else {
                debug!(
                    recipe = recipe.recipe_display_id,
                    "PlaceRecipe ignored — no world configured"
                );
            }
        }
        ServerboundSelectTrade::ID => {
            let selection = ServerboundSelectTrade::decode(&mut body)?;
            if let Some(state) = interaction.as_deref_mut() {
                handle_select_trade(state, writer, selection).await?;
            } else {
                debug!(
                    offer_index = selection.offer_index,
                    "SelectTrade ignored - no world configured"
                );
            }
        }
        ServerboundContainerButtonClick::ID => {
            let click = ServerboundContainerButtonClick::decode(&mut body)?;
            if let Some(state) = interaction.as_deref_mut() {
                handle_container_button_click(
                    state,
                    writer,
                    game_mode,
                    survival_state,
                    xp_state,
                    player_pose,
                    click,
                )
                .await?;
            } else {
                debug!(
                    container_id = click.container_id,
                    button_id = click.button_id,
                    "ContainerButtonClick ignored - no world configured"
                );
            }
        }
        ServerboundContainerClick::ID => {
            let click = ServerboundContainerClick::decode(&mut body)?;
            if let Some(state) = interaction.as_deref_mut() {
                // Keep the large click future out of the enclosing ingress/play-loop frames.
                Box::pin(handle_container_click(
                    state,
                    writer,
                    ContainerClickContext {
                        game_mode,
                        survival_state: *survival_state,
                        xp_state: &*xp_state,
                        player_pose,
                        script_events: script_gameplay_events,
                        scripts,
                        script_player_id: ScriptPlayerId::new(session_id),
                        script_context: script_player_context_from_values(
                            player_uuid,
                            player_name,
                            permissions,
                            player_pose,
                        ),
                    },
                    click,
                ))
                .await?;
            } else {
                debug!(
                    container_id = click.container_id,
                    slot = click.slot_num,
                    "ContainerClick ignored — no world configured"
                );
            }
        }
        ServerboundContainerClose::ID => {
            let close = ServerboundContainerClose::decode(&mut body)?;
            if let Some(state) = interaction {
                handle_client_close(state, writer, close.container_id, player_pose).await?;
            }
            debug!(
                container_id = close.container_id,
                "container close acknowledged"
            );
        }
        ServerboundSetCreativeModeSlot::ID => {
            let packet = ServerboundSetCreativeModeSlot::decode(&mut body)?;
            session::handle_creative_mode_slot(interaction, writer, game_mode, player_pose, packet)
                .await?;
        }
        _ => unreachable!("container helper only accepts container packet ids"),
    }
    Ok(())
}
