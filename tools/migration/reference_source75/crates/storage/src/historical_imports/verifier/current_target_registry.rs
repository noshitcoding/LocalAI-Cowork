//! Compiled exact registrations. Neither caller fields nor schema MAX(version)
//! may choose expected bytes or create a new supported cold-target profile.
use super::{
    Error, Result, canonical as c,
    target_profiles_current::{self, CurrentTargetProfile},
};
use serde_json::Value;

pub(super) fn select(descriptor: &Value) -> Result<CurrentTargetProfile> {
    let profile = match c::string(&descriptor["profile"])? {
        "v2-storage-71-complete-store-connect/1" => target_profiles_current::verified(
            include_bytes!("data/current-target-71.json"),
            "9a9ed9622c45ca0149f3a927dd5d5a3acb03d60bb07a3ea0f1bb6e007f3c1efd",
        )?,
        "v2-storage-67-complete-store-connect/1" => target_profiles_current::verified(
            include_bytes!("data/current-target-67.json"),
            "7d73781783626a3ec58a9e04173dc61c32f01a2f156ae09ac5fff52855695f49",
        )?,
        "v2-storage-66-complete-store-connect/1" => target_profiles_current::verified(
            include_bytes!("data/current-target-66.json"),
            "b6aa739bf422a550a0de001df4617f5ab11e9e4ec02fb142daa46f58ee775ac7",
        )?,
        "v2-storage-64-complete-store-connect/1" => target_profiles_current::verified(
            include_bytes!("data/current-target-64.json"),
            "910bc94e96a5753160cc53ca525c90b1836fd37013b9063f6885796053d77aa3",
        )?,
        _ => {
            return Err(Error::Refused(
                "Current cold-target profile has no exact registration",
            ));
        }
    };
    if *descriptor != profile.descriptor() {
        return Err(Error::Refused(
            "Preview current target descriptor differs from fixed profile",
        ));
    }
    Ok(profile)
}
