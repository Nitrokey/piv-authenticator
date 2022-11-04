//! Parsed PIV commands.
//!
//! The types here should enforce all restrictions in the spec (such as padded_piv_pin.len() == 8),
//! but no implementation-specific ones (such as "GlobalPin not supported").

use core::convert::{TryFrom, TryInto};

// use flexiber::Decodable;
use iso7816::{Instruction, Status};

use crate::state::TouchPolicy;
pub use crate::{
    container::{
        self as containers, AuthenticateKeyReference, ChangeReferenceKeyReference,
        GenerateAsymmetricKeyReference, VerifyKeyReference,
    },
    piv_types, Pin, Puk,
};

// https://developers.yubico.com/PIV/Introduction/Yubico_extensions.html
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum YubicoPivExtension {
    SetManagementKey(TouchPolicy),
    ImportAsymmetricKey,
    GetVersion,
    Reset,
    SetPinRetries,
    Attest,
    GetSerial, // also used via 0x01
    GetMetadata,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command<'l> {
    /// Select the application
    ///
    /// Resets security indicators if we are implicitly deselected.
    Select(Select<'l>),
    /// Get a data object / container.
    GetData(containers::Container),
    /// Check PIN
    ///
    /// This verifies that the sent PIN (global or PIV) is correct.
    ///
    /// In principle, other key references (biometric, pairing code) could
    /// be verified, but this is not implemented.
    Verify(Verify),
    /// Change PIN or PUK
    ChangeReference(ChangeReference),
    /// If the PIN is blocked, reset it using the PUK
    ResetPinRetries(ResetPinRetries),
    /// The most general purpose method, performing actual cryptographic operations
    ///
    /// In particular, this can also decrypt or similar.
    GeneralAuthenticate(GeneralAuthenticate),
    /// Store a data object / container.
    PutData(PutData),
    GenerateAsymmetric(GenerateAsymmetric),

    /* Yubico commands */
    YkExtension(YubicoPivExtension),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GeneralAuthenticate {
    algorithm: piv_types::Algorithms,
    key_reference: AuthenticateKeyReference,
}

impl<'l> Command<'l> {
    /// Core method, constructs a PIV command, if the iso7816::Command is valid.
    ///
    /// Inherent method re-exposing the `TryFrom` implementation.
    pub fn try_from<const C: usize>(command: &'l iso7816::Command<C>) -> Result<Self, Status> {
        command.try_into()
    }
}

/// TODO: change into enum
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Select<'l> {
    pub aid: &'l [u8],
}

impl<'l> TryFrom<&'l [u8]> for Select<'l> {
    type Error = Status;
    /// We allow ourselves the option of answering to more than just the official PIV AID.
    /// For instance, to offer additional functionality, under our own RID.
    fn try_from(data: &'l [u8]) -> Result<Self, Self::Error> {
        if crate::constants::PIV_AID.matches(data) {
            Ok(Self { aid: data })
        } else {
            Err(Status::NotFound)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GetData(containers::Container);

impl TryFrom<&[u8]> for GetData {
    type Error = Status;
    fn try_from(data: &[u8]) -> Result<Self, Self::Error> {
        let mut decoder = flexiber::Decoder::new(data);
        let tagged_slice: flexiber::TaggedSlice = decoder
            .decode()
            .map_err(|_| Status::IncorrectDataParameter)?;
        if tagged_slice.tag() != flexiber::Tag::application(0x1C) {
            return Err(Status::IncorrectDataParameter);
        }
        let container: containers::Container = containers::Tag::new(tagged_slice.as_bytes())
            .try_into()
            .map_err(|_| Status::IncorrectDataParameter)?;

        info!("request to GetData for container {:?}", container);
        Ok(Self(container))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerifyLogout(bool);

impl TryFrom<u8> for VerifyLogout {
    type Error = Status;
    fn try_from(p1: u8) -> Result<Self, Self::Error> {
        match p1 {
            0x00 => Ok(Self(false)),
            0xFF => Ok(Self(true)),
            _ => Err(Status::IncorrectP1OrP2Parameter),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerifyArguments<'l> {
    pub key_reference: VerifyKeyReference,
    pub logout: VerifyLogout,
    pub data: &'l [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum VerifyLogin {
    PivPin(Pin),
    GlobalPin([u8; 8]),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verify {
    Login(VerifyLogin),
    Logout(VerifyKeyReference),
    Status(VerifyKeyReference),
}

impl TryFrom<VerifyArguments<'_>> for Verify {
    type Error = Status;
    fn try_from(arguments: VerifyArguments<'_>) -> Result<Self, Self::Error> {
        let VerifyArguments {
            key_reference,
            logout,
            data,
        } = arguments;
        if key_reference != VerifyKeyReference::ApplicationPin {
            return Err(Status::FunctionNotSupported);
        }
        Ok(match (logout.0, data.len()) {
            (false, 0) => Verify::Status(key_reference),
            (false, 8) => Verify::Login(VerifyLogin::PivPin(
                data.try_into()
                    .map_err(|_| Status::IncorrectDataParameter)?,
            )),
            (false, _) => return Err(Status::IncorrectDataParameter),
            (true, 0) => Verify::Logout(key_reference),
            (true, _) => return Err(Status::IncorrectDataParameter),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChangeReferenceArguments<'l> {
    pub key_reference: ChangeReferenceKeyReference,
    pub data: &'l [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangeReference {
    ChangePin { old_pin: Pin, new_pin: Pin },
    ChangePuk { old_puk: Puk, new_puk: Puk },
}

impl TryFrom<ChangeReferenceArguments<'_>> for ChangeReference {
    type Error = Status;
    fn try_from(arguments: ChangeReferenceArguments<'_>) -> Result<Self, Self::Error> {
        let ChangeReferenceArguments {
            key_reference,
            data,
        } = arguments;

        use ChangeReferenceKeyReference::*;
        Ok(match (key_reference, data) {
            (GlobalPin, _) => return Err(Status::FunctionNotSupported),
            (ApplicationPin, data) => ChangeReference::ChangePin {
                old_pin: Pin::try_from(&data[..8]).map_err(|_| Status::IncorrectDataParameter)?,
                new_pin: Pin::try_from(&data[8..]).map_err(|_| Status::IncorrectDataParameter)?,
            },
            (PinUnblockingKey, data) => ChangeReference::ChangePuk {
                old_puk: Puk(data[..8]
                    .try_into()
                    .map_err(|_| Status::IncorrectDataParameter)?),
                new_puk: Puk(data[8..]
                    .try_into()
                    .map_err(|_| Status::IncorrectDataParameter)?),
            },
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResetPinRetries {
    pub padded_pin: [u8; 8],
    pub puk: [u8; 8],
}

impl TryFrom<&[u8]> for ResetPinRetries {
    type Error = Status;
    fn try_from(data: &[u8]) -> Result<Self, Self::Error> {
        if data.len() != 16 {
            return Err(Status::IncorrectDataParameter);
        }
        Ok(Self {
            padded_pin: data[..8].try_into().unwrap(),
            puk: data[8..].try_into().unwrap(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthenticateArguments<'l> {
    /// To allow the authenticator to have additional algorithms beyond NIST SP 800-78-4,
    /// this is passed through as-is.
    pub unparsed_algorithm: u8,
    pub key_reference: AuthenticateKeyReference,
    pub data: &'l [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PutData {}

impl TryFrom<&[u8]> for PutData {
    type Error = Status;
    fn try_from(_data: &[u8]) -> Result<Self, Self::Error> {
        todo!();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GenerateAsymmetricArguments<'l> {
    pub key_reference: GenerateAsymmetricKeyReference,
    pub data: &'l [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenerateAsymmetric {}

impl TryFrom<GenerateAsymmetricArguments<'_>> for GenerateAsymmetric {
    type Error = Status;
    fn try_from(_arguments: GenerateAsymmetricArguments<'_>) -> Result<Self, Self::Error> {
        todo!();
    }
}

impl<'l, const C: usize> TryFrom<&'l iso7816::Command<C>> for Command<'l> {
    type Error = Status;
    /// The first layer of unraveling the iso7816::Command onion.
    ///
    /// The responsibility here is to check (cla, ins, p1, p2) are valid as defined
    /// in the "Command Syntax" boxes of NIST SP 800-73-4, and return early errors.
    ///
    /// The individual piv::Command TryFroms then further interpret these validated parameters.
    fn try_from(command: &'l iso7816::Command<C>) -> Result<Self, Self::Error> {
        let (class, instruction, p1, p2) = (
            command.class(),
            command.instruction(),
            command.p1,
            command.p2,
        );
        let data = command.data();

        if !class.secure_messaging().none() {
            return Err(Status::SecureMessagingNotSupported);
        }

        if class.channel() != Some(0) {
            return Err(Status::LogicalChannelNotSupported);
        }

        // TODO: should we check `command.expected() == 0`, where specified?

        Ok(match (class.into_inner(), instruction, p1, p2) {
            (0x00, Instruction::Select, 0x04, 0x00) => {
                Self::Select(Select::try_from(data.as_slice())?)
            }

            (0x00, Instruction::GetData, 0x3F, 0xFF) => {
                Self::GetData(GetData::try_from(data.as_slice())?.0)
            }

            (0x00, Instruction::Verify, p1, p2) => {
                let logout = VerifyLogout::try_from(p1)?;
                let key_reference = VerifyKeyReference::try_from(p2)?;
                Self::Verify(Verify::try_from(VerifyArguments {
                    key_reference,
                    logout,
                    data,
                })?)
            }

            (0x00, Instruction::ChangeReferenceData, 0x00, p2) => {
                let key_reference = ChangeReferenceKeyReference::try_from(p2)?;
                Self::ChangeReference(ChangeReference::try_from(ChangeReferenceArguments {
                    key_reference,
                    data,
                })?)
            }

            (0x00, Instruction::ResetRetryCounter, 0x00, 0x80) => {
                Self::ResetPinRetries(ResetPinRetries::try_from(data.as_slice())?)
            }

            (0x00, Instruction::GeneralAuthenticate, p1, p2) => {
                let algorithm = p1.try_into()?;
                let key_reference = AuthenticateKeyReference::try_from(p2)?;
                Self::GeneralAuthenticate(GeneralAuthenticate {
                    algorithm,
                    key_reference,
                })
            }

            (0x00, Instruction::PutData, 0x3F, 0xFF) => {
                Self::PutData(PutData::try_from(data.as_slice())?)
            }

            (0x00, Instruction::GenerateAsymmetricKeyPair, 0x00, p2) => {
                let key_reference = GenerateAsymmetricKeyReference::try_from(p2)?;
                Self::GenerateAsymmetric(GenerateAsymmetric::try_from(
                    GenerateAsymmetricArguments {
                        key_reference,
                        data,
                    },
                )?)
            }
            // (0x00, 0x01, 0x10, 0x00)
            (0x00, Instruction::Unknown(0x01), 0x00, 0x00) => {
                Self::YkExtension(YubicoPivExtension::GetSerial)
            }
            (0x00, Instruction::Unknown(0xff), 0xFF, 0xFE) => {
                Self::YkExtension(YubicoPivExtension::SetManagementKey(TouchPolicy::Never))
            }
            (0x00, Instruction::Unknown(0xff), 0xFF, 0xFF) => {
                Self::YkExtension(YubicoPivExtension::SetManagementKey(TouchPolicy::Always))
            }
            (0x00, Instruction::Unknown(0xfe), 0x00, 0x00) => {
                Self::YkExtension(YubicoPivExtension::ImportAsymmetricKey)
            }
            (0x00, Instruction::Unknown(0xfd), 0x00, 0x00) => {
                Self::YkExtension(YubicoPivExtension::GetVersion)
            }
            (0x00, Instruction::Unknown(0xfb), 0x00, 0x00) => {
                Self::YkExtension(YubicoPivExtension::Reset)
            }
            (0x00, Instruction::Unknown(0xfa), 0x00, 0x00) => {
                Self::YkExtension(YubicoPivExtension::SetPinRetries)
            }
            // (0x00, 0xf9, 0x9a, 0x00)
            (0x00, Instruction::Unknown(0xf9), _, _) => {
                Self::YkExtension(YubicoPivExtension::Attest)
            }
            // (0x00, 0xf8, 0x00, 0x00)
            (0x00, Instruction::Unknown(0xf8), _, _) => {
                Self::YkExtension(YubicoPivExtension::GetSerial)
            }
            (0x00, Instruction::Unknown(0xf7), _, _) => {
                Self::YkExtension(YubicoPivExtension::GetMetadata)
            }

            _ => return Err(Status::FunctionNotSupported),
        })
    }
}
