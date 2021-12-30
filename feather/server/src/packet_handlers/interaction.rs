use crate::{ClientId, NetworkId, Server};
use base::{BlockKind, ItemStack, Item};
use base::inventory::{SLOT_HOTBAR_OFFSET, SLOT_OFFHAND};
use common::entities::player::HotbarSlot;
use common::interactable::InteractableRegistry;
use common::{Game, Window};
use ecs::{Entity, EntityRef, SysResult};
use libcraft_core::{BlockFace as LibcraftBlockFace, Hand};
use libcraft_core::{InteractionType, Vec3f};
use protocol::packets::client::{
    BlockFace, HeldItemChange, InteractEntity, InteractEntityKind, PlayerBlockPlacement,
    PlayerDigging, PlayerDiggingStatus,
};
use quill_common::{
    events::{BlockInteractEvent, BlockPlacementEvent, InteractEntityEvent},
    EntityId,
};
/// Handles the player block placement packet. Currently just removes the block client side for the player.
pub fn handle_player_block_placement(
    game: &mut Game,
    _server: &mut Server,
    packet: PlayerBlockPlacement,
    player: Entity,
) -> SysResult {
    let hand = match packet.hand {
        0 => Hand::Main,
        1 => Hand::Offhand,
        _ => {
            let client_id = game.ecs.get::<ClientId>(player).unwrap();

            let client = _server.clients.get(*client_id).unwrap();

            client.disconnect("Malformed Packet!");

            anyhow::bail!(
                "Player sent a malformed `PlayerBlockPlacement` packet. {:?}",
                packet
            )
        }
    };

    let face = match packet.face {
        BlockFace::North => LibcraftBlockFace::North,
        BlockFace::South => LibcraftBlockFace::South,
        BlockFace::East => LibcraftBlockFace::East,
        BlockFace::West => LibcraftBlockFace::West,
        BlockFace::Top => LibcraftBlockFace::Top,
        BlockFace::Bottom => LibcraftBlockFace::Bottom,
    };

    let cursor_position = Vec3f::new(
        packet.cursor_position_x,
        packet.cursor_position_y,
        packet.cursor_position_z,
    );

    let block_kind = {
        let result = game.block(packet.position);
        match result {
            Some(block) => block.kind(),
            None => {
                let client_id = game.ecs.get::<ClientId>(player).unwrap();

                let client = _server.clients.get(*client_id).unwrap();

                client.disconnect("Attempted to interact with an unloaded block!");

                anyhow::bail!(
                    "Player attempted to interact with an unloaded block. {:?}",
                    packet
                )
            }
        }
    };

    let interactable_registry = game
        .resources
        .get::<InteractableRegistry>()
        .expect("Failed to get the interactable registry");

    if interactable_registry.is_registered(block_kind) {
        // Handle this as a block interaction
        let event = BlockInteractEvent {
            hand,
            location: packet.position.into(),
            face,
            cursor_position,
            inside_block: packet.inside_block,
        };

        game.ecs.insert_entity_event(player, event)?;
    } else {
        // Handle this as a block placement
        let event = BlockPlacementEvent {
            hand,
            location: packet.position.into(),
            face,
            cursor_position,
            inside_block: packet.inside_block,
        };

        game.ecs.insert_entity_event(player, event)?;
    }

    Ok(())
}

/// Handles the Player Digging packet sent for the following
/// actions:
/// * Breaking blocks.
/// * Dropping items.
/// * Shooting arrows.
/// * Eating.
/// * Swapping items between the main and off hand.
pub fn handle_player_digging(
    game: &mut Game,
    server: &mut Server,
    packet: PlayerDigging,
    player: Entity,
) -> SysResult {
    log::trace!("Got player digging with status {:?}", packet.status);
    match packet.status {
        PlayerDiggingStatus::StartDigging | PlayerDiggingStatus::CancelDigging => {
            let block = game.block(packet.position).unwrap();
            
            let window = game.ecs.get::<Window>(player)?;

            let hotbar_slot = game.ecs.get::<HotbarSlot>(player)?.get();

            let hotbar_index = SLOT_HOTBAR_OFFSET + hotbar_slot;
            let offhand_index = SLOT_OFFHAND;

            let mut hotbar_item = window.item(hotbar_index)?;
            
            let breaking_speed = block_breaking_speed(block.kind(),hotbar_item.item_kind());
            let speed_in_ticks = if breaking_speed > 1.0 {
                0
            } else{
                (1.0/breaking_speed) as u32
            };
            

            Ok(())
        }
        PlayerDiggingStatus::SwapItemInHand => {
            let window = game.ecs.get::<Window>(player)?;

            let hotbar_slot = game.ecs.get::<HotbarSlot>(player)?.get();

            let hotbar_index = SLOT_HOTBAR_OFFSET + hotbar_slot;
            let offhand_index = SLOT_OFFHAND;

            {
                let mut hotbar_item = window.item(hotbar_index)?;
                let mut offhand_item = window.item(offhand_index)?;

                std::mem::swap(&mut *hotbar_item, &mut *offhand_item);
            }

            let client_id = *game.ecs.get::<ClientId>(player)?;
            let client = server.clients.get(client_id).unwrap();

            client.send_window_items(&window);

            Ok(())
        }
        _ => Ok(()),
    }
}

pub fn block_breaking_speed(block_kind: BlockKind, held_item: Option<Item>) -> f32{
    let harvest_tools = block_kind.harvest_tools();
    let hardeness = block_kind.hardness();
    let dig_multipliers = block_kind.dig_multipliers();

    let adjusted_hardness = match harvest_tools{
        Some(tools) => {
            if held_item.is_none(){
                5.0
            } else if tools.contains(held_item.as_ref().unwrap()){
                1.5
            } else {
                5.0
            }
        },
        None => {
            1.5
        },
    }*hardeness;


    let mut speed_multiplier = if held_item.is_none(){
        1.0
    } else {
        dig_multipliers.iter()
        .find(|(i,h)|{*i==held_item.unwrap()}).map(|(_,h)|*h).unwrap_or(1.0)
    };
    
    
    //TODO: get efficiency from item metadata 
    let ef_level = 0;
    if ef_level != 0{
        speed_multiplier += (ef_level*ef_level + 1) as f32;
    }
    speed_multiplier/adjusted_hardness
}

pub fn handle_interact_entity(
    game: &mut Game,
    _server: &mut Server,
    packet: InteractEntity,
    player: Entity,
) -> SysResult {
    let target = {
        let mut found_entity = None;
        for (entity, &network_id) in game.ecs.query::<&NetworkId>().iter() {
            if network_id.0 == packet.entity_id {
                found_entity = Some(entity);
                break;
            }
        }

        match found_entity {
            None => {
                let client_id = game.ecs.get::<ClientId>(player).unwrap();

                let client = _server.clients.get(*client_id).unwrap();

                client.disconnect("Interacted with an invalid entity!");

                anyhow::bail!("Player attempted to interact with an invalid entity.")
            }
            Some(entity) => entity,
        }
    };

    let event = match packet.kind {
        InteractEntityKind::Attack => InteractEntityEvent {
            target: EntityId(target.id() as u64),
            ty: InteractionType::Attack,
            target_pos: None,
            hand: None,
            sneaking: packet.sneaking,
        },
        InteractEntityKind::Interact => InteractEntityEvent {
            target: EntityId(target.id() as u64),
            ty: InteractionType::Interact,
            target_pos: None,
            hand: None,
            sneaking: packet.sneaking,
        },
        InteractEntityKind::InteractAt {
            target_x,
            target_y,
            target_z,
            hand,
        } => {
            let hand = match hand {
                0 => Hand::Main,
                1 => Hand::Offhand,
                _ => unreachable!(),
            };

            InteractEntityEvent {
                target: EntityId(target.id() as u64),
                ty: InteractionType::Attack,
                target_pos: Some(Vec3f::new(
                    target_x as f32,
                    target_y as f32,
                    target_z as f32,
                )),
                hand: Some(hand),
                sneaking: packet.sneaking,
            }
        }
    };

    game.ecs.insert_entity_event(player, event)?;

    Ok(())
}

pub fn handle_held_item_change(player: EntityRef, packet: HeldItemChange) -> SysResult {
    let new_id = packet.slot as usize;
    let mut slot = player.get_mut::<HotbarSlot>()?;

    log::trace!("Got player slot change from {} to {}", slot.get(), new_id);

    slot.set(new_id)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use common::Game;
    use protocol::packets::client::HeldItemChange;

    use super::*;

    #[test]
    fn held_item_change() {
        let mut game = Game::new();
        let entity = game.ecs.spawn((HotbarSlot::new(0),));
        let player = game.ecs.entity(entity).unwrap();

        let packet = HeldItemChange { slot: 8 };

        handle_held_item_change(player, packet).unwrap();

        assert_eq!(
            *game.ecs.get::<HotbarSlot>(entity).unwrap(),
            HotbarSlot::new(8)
        );
    }
}
