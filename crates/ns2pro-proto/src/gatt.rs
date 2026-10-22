//! The controller's GATT characteristics.

use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Characteristic {
    /// Written once with `[0x01, 0x00]` to start the session.
    Init,
    /// Notifies common input reports.
    Input,
    /// Accepts 42-byte rumble packets.
    Vibration,
    /// Accepts protocol commands.
    Command,
    /// Notifies one response per command.
    CommandResponse,
}

impl Characteristic {
    pub const ALL: [Self; 5] = [Self::Init, Self::Input, Self::Vibration, Self::Command, Self::CommandResponse];

    /// Characteristics the bridge subscribes to.
    pub const NOTIFYING: [Self; 2] = [Self::Input, Self::CommandResponse];

    pub const fn uuid(self) -> Uuid {
        match self {
            Self::Init => Uuid::from_u128(0x00c5af5d_1964_4e30_8f51_1956f96bd282),
            Self::Input => Uuid::from_u128(0xab7de9be_89fe_49ad_828f_118f09df7fd2),
            Self::Vibration => Uuid::from_u128(0xcc483f51_9258_427d_a939_630c31f72b05),
            Self::Command => Uuid::from_u128(0x649d4ac9_8eb7_4e6c_af44_1ea54fe5f005),
            Self::CommandResponse => Uuid::from_u128(0xc765a961_d9d8_4d36_a20a_5315b111836a),
        }
    }

    pub fn from_uuid(uuid: Uuid) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.uuid() == uuid)
    }
}

/// Payload written to [`Characteristic::Init`].
pub const INIT_PAYLOAD: [u8; 2] = [0x01, 0x00];
