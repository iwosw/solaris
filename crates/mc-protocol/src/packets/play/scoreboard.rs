//! Vanilla 26.1.2 scoreboard packets, from the patched 26.1.2.76 source jar.
//! IDs count GameProtocols.CLIENTBOUND_TEMPLATE registrations, including bundle
//! delimiter at zero. Components use the trusted network-NBT stream codec.

use super::*;

#[derive(Debug, Clone, PartialEq)]
pub struct TeamParameters {
    pub display_name: Tag,
    pub options: u8,
    pub nametag_visibility: i32,
    pub collision_rule: i32,
    pub color: i32,
    pub prefix: Tag,
    pub suffix: Tag,
}

impl TeamParameters {
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        mc_nbt::write_network_value(buf, &self.display_name)?;
        buf.write_u8(self.options);
        // Team.Visibility and Team.CollisionRule use ByteBufCodecs.idMapper,
        // not the legacy UTF names. ChatFormatting uses writeEnum (VarInt).
        buf.write_varint(self.nametag_visibility);
        buf.write_varint(self.collision_rule);
        buf.write_varint(self.color);
        mc_nbt::write_network_value(buf, &self.prefix)?;
        mc_nbt::write_network_value(buf, &self.suffix)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            display_name: mc_nbt::read_network_value(buf)?,
            options: buf.read_u8()?,
            nametag_visibility: buf.read_varint()?,
            collision_rule: buf.read_varint()?,
            color: buf.read_varint()?,
            prefix: mc_nbt::read_network_value(buf)?,
            suffix: mc_nbt::read_network_value(buf)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundSetPlayerTeam {
    pub name: String,
    pub method: u8,
    pub parameters: Option<TeamParameters>,
    pub players: Vec<String>,
}

impl Packet for ClientboundSetPlayerTeam {
    // GamePacketTypes.CLIENTBOUND_SET_PLAYER_TEAM, registration 109.
    const ID: i32 = 0x6D;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.method > 4 {
            return Err(CodecError::NotSupported("invalid team method"));
        }
        let parameters = matches!(self.method, 0 | 2);
        let players = matches!(self.method, 0 | 3 | 4);
        if parameters != self.parameters.is_some() || (!players && !self.players.is_empty()) {
            return Err(CodecError::NotSupported(
                "team fields inconsistent with method",
            ));
        }
        let count = i32::try_from(self.players.len())
            .map_err(|_| CodecError::NotSupported("team player count exceeds VarInt"))?;
        buf.write_string(&self.name, DEFAULT_MAX_STRING_LEN)?;
        buf.write_u8(self.method);
        if let Some(parameters) = &self.parameters {
            parameters.encode(buf)?;
        }
        if players {
            buf.write_varint(count);
            for player in &self.players {
                buf.write_string(player, DEFAULT_MAX_STRING_LEN)?;
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let name = buf.read_string(DEFAULT_MAX_STRING_LEN)?;
        let method = buf.read_u8()?;
        if method > 4 {
            return Err(CodecError::NotSupported("invalid team method"));
        }
        let parameters = if matches!(method, 0 | 2) {
            Some(TeamParameters::decode(buf)?)
        } else {
            None
        };
        let mut players = Vec::new();
        if matches!(method, 0 | 3 | 4) {
            let count = read_bounded_count(buf, i32::MAX as usize, 1)?;
            players = bounded_vec(count);
            for _ in 0..count {
                players.push(buf.read_string(DEFAULT_MAX_STRING_LEN)?);
            }
        }
        Ok(Self {
            name,
            method,
            parameters,
            players,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundSetObjective {
    pub name: String,
    pub method: u8,
    pub display_name: Option<Tag>,
    pub render_type: i32,
}

impl Packet for ClientboundSetObjective {
    // GamePacketTypes.CLIENTBOUND_SET_OBJECTIVE, registration 106.
    const ID: i32 = 0x6A;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.method > 2 {
            return Err(CodecError::NotSupported("invalid objective method"));
        }
        if (self.method != 1) != self.display_name.is_some() {
            return Err(CodecError::NotSupported(
                "objective display inconsistent with method",
            ));
        }
        buf.write_string(&self.name, DEFAULT_MAX_STRING_LEN)?;
        buf.write_u8(self.method);
        if let Some(display_name) = &self.display_name {
            mc_nbt::write_network_value(buf, display_name)?;
            buf.write_varint(self.render_type);
            buf.write_bool(false); // Optional<NumberFormat> absent.
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let name = buf.read_string(DEFAULT_MAX_STRING_LEN)?;
        let method = buf.read_u8()?;
        if method > 2 {
            return Err(CodecError::NotSupported("invalid objective method"));
        }
        let (display_name, render_type) = if method != 1 {
            let display_name = mc_nbt::read_network_value(buf)?;
            let render_type = buf.read_varint()?;
            if buf.read_bool()? {
                return Err(CodecError::NotSupported("objective number format"));
            }
            (Some(display_name), render_type)
        } else {
            (None, 0)
        };
        Ok(Self {
            name,
            method,
            display_name,
            render_type,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundSetScore {
    pub owner: String,
    pub objective: String,
    pub score: i32,
}

impl Packet for ClientboundSetScore {
    // GamePacketTypes.CLIENTBOUND_SET_SCORE, registration 110.
    const ID: i32 = 0x6E;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_string(&self.owner, DEFAULT_MAX_STRING_LEN)?;
        buf.write_string(&self.objective, DEFAULT_MAX_STRING_LEN)?;
        buf.write_varint(self.score);
        buf.write_bool(false); // Optional<Component> display absent.
        buf.write_bool(false); // Optional<NumberFormat> absent.
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let owner = buf.read_string(DEFAULT_MAX_STRING_LEN)?;
        let objective = buf.read_string(DEFAULT_MAX_STRING_LEN)?;
        let score = buf.read_varint()?;
        if buf.read_bool()? {
            return Err(CodecError::NotSupported("score display component"));
        }
        if buf.read_bool()? {
            return Err(CodecError::NotSupported("score number format"));
        }
        Ok(Self {
            owner,
            objective,
            score,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundResetScore {
    pub owner: String,
    pub objective: Option<String>,
}

impl Packet for ClientboundResetScore {
    // GamePacketTypes.CLIENTBOUND_RESET_SCORE, registration 79.
    const ID: i32 = 0x4F;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_string(&self.owner, DEFAULT_MAX_STRING_LEN)?;
        buf.write_bool(self.objective.is_some());
        if let Some(objective) = &self.objective {
            buf.write_string(objective, DEFAULT_MAX_STRING_LEN)?;
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let owner = buf.read_string(DEFAULT_MAX_STRING_LEN)?;
        let objective = if buf.read_bool()? {
            Some(buf.read_string(DEFAULT_MAX_STRING_LEN)?)
        } else {
            None
        };
        Ok(Self { owner, objective })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundSetDisplayObjective {
    pub slot: i32,
    pub objective: String,
}

impl Packet for ClientboundSetDisplayObjective {
    // GamePacketTypes.CLIENTBOUND_SET_DISPLAY_OBJECTIVE, registration 98.
    const ID: i32 = 0x62;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.slot); // FriendlyByteBuf.writeById uses VarInt.
        buf.write_string(&self.objective, DEFAULT_MAX_STRING_LEN)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            slot: buf.read_varint()?,
            objective: buf.read_string(DEFAULT_MAX_STRING_LEN)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture<P: Packet + PartialEq + std::fmt::Debug>(packet: P, wire: &[u8]) {
        let mut input = wire;
        assert_eq!(P::decode(&mut input).unwrap(), packet);
        assert!(input.is_empty());
        let mut encoded = Vec::new();
        packet.encode(&mut encoded).unwrap();
        assert_eq!(encoded, wire);
    }

    #[test]
    fn vanilla_registration_ids() {
        assert_eq!(ClientboundSetPlayerTeam::ID, 109);
        assert_eq!(ClientboundSetObjective::ID, 106);
        assert_eq!(ClientboundSetScore::ID, 110);
        assert_eq!(ClientboundResetScore::ID, 79);
        assert_eq!(ClientboundSetDisplayObjective::ID, 98);
    }

    #[test]
    fn team_wire_all_methods_and_varint_enums() {
        let parameters = TeamParameters {
            display_name: Tag::String("D".into()),
            options: 3,
            nametag_visibility: 2,
            collision_rule: 3,
            color: 21,
            prefix: Tag::String("P".into()),
            suffix: Tag::String("S".into()),
        };
        for method in 0..=4 {
            let has_parameters = matches!(method, 0 | 2);
            let has_players = matches!(method, 0 | 3 | 4);
            let mut wire = vec![1, b't', method];
            if has_parameters {
                wire.extend_from_slice(&[8, 0, 1, b'D', 3, 2, 3, 21, 8, 0, 1, b'P', 8, 0, 1, b'S']);
            }
            if has_players {
                wire.extend_from_slice(&[1, 1, b'p']);
            }
            fixture(
                ClientboundSetPlayerTeam {
                    name: "t".into(),
                    method,
                    parameters: has_parameters.then(|| parameters.clone()),
                    players: if has_players {
                        vec!["p".into()]
                    } else {
                        vec![]
                    },
                },
                &wire,
            );
        }
    }

    #[test]
    fn objective_wire_all_methods() {
        for method in 0..=2 {
            let mut wire = vec![1, b'o', method];
            if method != 1 {
                wire.extend_from_slice(&[8, 0, 1, b'D', 1, 0]);
            }
            fixture(
                ClientboundSetObjective {
                    name: "o".into(),
                    method,
                    display_name: (method != 1).then(|| Tag::String("D".into())),
                    render_type: if method == 1 { 0 } else { 1 },
                },
                &wire,
            );
        }
    }

    #[test]
    fn score_reset_and_display_wire() {
        fixture(
            ClientboundSetScore {
                owner: "p".into(),
                objective: "o".into(),
                score: -1,
            },
            &[1, b'p', 1, b'o', 255, 255, 255, 255, 15, 0, 0],
        );
        fixture(
            ClientboundResetScore {
                owner: "p".into(),
                objective: None,
            },
            &[1, b'p', 0],
        );
        fixture(
            ClientboundResetScore {
                owner: "p".into(),
                objective: Some("o".into()),
            },
            &[1, b'p', 1, 1, b'o'],
        );
        fixture(
            ClientboundSetDisplayObjective {
                slot: 18,
                objective: "o".into(),
            },
            &[18, 1, b'o'],
        );
        fixture(
            ClientboundSetDisplayObjective {
                slot: 1,
                objective: String::new(),
            },
            &[1, 0],
        );
    }

    #[test]
    fn rejects_invalid_methods_and_inconsistent_fields() {
        for method in [5, 255] {
            assert!(ClientboundSetPlayerTeam::decode(&mut &[1, b't', method][..]).is_err());
            assert!(
                ClientboundSetPlayerTeam {
                    name: "t".into(),
                    method,
                    parameters: None,
                    players: vec![]
                }
                .encode(&mut Vec::new())
                .is_err()
            );
        }
        for method in [3, 255] {
            assert!(ClientboundSetObjective::decode(&mut &[1, b'o', method][..]).is_err());
            assert!(
                ClientboundSetObjective {
                    name: "o".into(),
                    method,
                    display_name: None,
                    render_type: 0
                }
                .encode(&mut Vec::new())
                .is_err()
            );
        }
        assert!(
            ClientboundSetPlayerTeam {
                name: "t".into(),
                method: 0,
                parameters: None,
                players: vec![]
            }
            .encode(&mut Vec::new())
            .is_err()
        );
        assert!(
            ClientboundSetPlayerTeam {
                name: "t".into(),
                method: 1,
                parameters: None,
                players: vec!["p".into()]
            }
            .encode(&mut Vec::new())
            .is_err()
        );
        assert!(
            ClientboundSetObjective {
                name: "o".into(),
                method: 0,
                display_name: None,
                render_type: 0
            }
            .encode(&mut Vec::new())
            .is_err()
        );
    }

    #[test]
    fn rejects_unsupported_optional_fields_and_truncation() {
        assert!(matches!(
            ClientboundSetObjective::decode(&mut &[1, b'o', 0, 8, 0, 1, b'D', 0, 1][..]),
            Err(CodecError::NotSupported(_))
        ));
        assert!(matches!(
            ClientboundSetScore::decode(&mut &[1, b'p', 1, b'o', 0, 1][..]),
            Err(CodecError::NotSupported(_))
        ));
        assert!(matches!(
            ClientboundSetScore::decode(&mut &[1, b'p', 1, b'o', 0, 0, 1][..]),
            Err(CodecError::NotSupported(_))
        ));
        assert!(ClientboundSetScore::decode(&mut &[1, b'p', 1, b'o', 0, 0][..]).is_err());
        assert!(ClientboundSetPlayerTeam::decode(&mut &[1, b't', 3, 2, 1, b'p'][..]).is_err());
    }
}
