pub mod audio;
pub mod config;
pub mod dbus;
pub mod openai_compat;
pub mod pipeline;
pub mod prompt;
pub mod protocol;
pub mod secrets;
pub mod vocabulary;
pub mod worker;

pub const BUS_NAME: &str = "io.github.ddy314.NatsuTypeless";
pub const OBJECT_PATH: &str = "/io/github/ddy314/NatsuTypeless";
pub const INTERFACE_NAME: &str = "io.github.ddy314.NatsuTypeless";
