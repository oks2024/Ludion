//! The `LUDION_REQUIRE_GPU` contract: a lane that gates images must not pass by skipping.
//!
//! GPU tests skip with a `SKIPPED` line when no adapter exists, so hosted CI and laptops stay
//! green. With `LUDION_REQUIRE_GPU=1` every such skip panics instead — set by `check.mjs`'s test
//! lane and the pinned CI agents, where a missing adapter means the box lost its GPU, not that the
//! test is optional. Every no-adapter skip in the workspace routes its detection through
//! [`skip_no_adapter`] or [`is_no_adapter`]; `scripts/lists-in-step.mjs` enforces it.
//!
//! The mirror image is `LUDION_FORBID_GPU=1`, set on the hosted `no-gpu` lane: device creation
//! ([`assert_gpu_allowed`]) panics, so a test that needs a device but is not named `needs_gpu_*`
//! (the name the `no-gpu` nextest profile excludes) fails loudly instead of rendering on a
//! software adapter.

use crate::codes::code;
use crate::diagnostics::Diagnostic;

/// The environment variable that turns a no-adapter skip into a failure.
pub const REQUIRE_GPU_ENV: &str = "LUDION_REQUIRE_GPU";

/// The environment variable that makes device creation panic.
pub const FORBID_GPU_ENV: &str = "LUDION_FORBID_GPU";

/// Parses the setting: `1` is true; unset, empty or `0` is false; anything else panics naming the
/// variable and the accepted values (a typo must not silently disable the gate).
pub fn require_gpu_setting(value: Option<&str>) -> bool {
    parse_setting(REQUIRE_GPU_ENV, value)
}

fn parse_setting(var: &str, value: Option<&str>) -> bool {
    match value {
        None | Some("") | Some("0") => false,
        Some("1") => true,
        Some(other) => {
            panic!("{var}={other:?} is not understood: set it to `1` (on) or `0` / unset it (off)")
        }
    }
}

/// Whether this process's lane requires a GPU adapter.
pub fn gpu_required() -> bool {
    require_gpu_setting(std::env::var(REQUIRE_GPU_ENV).ok().as_deref())
}

/// The gate for a no-adapter skip: returns `no_adapter`, but panics when it is true and the lane
/// requires a GPU.
pub fn skip_no_adapter(no_adapter: bool) -> bool {
    gate(no_adapter, gpu_required())
}

/// Whether `diags` carry `L2003` (no GPU adapter), through [`skip_no_adapter`].
pub fn is_no_adapter(diags: &[Diagnostic]) -> bool {
    skip_no_adapter(diags.iter().any(|d| d.code == code::RENDER_NO_ADAPTER))
}

/// Called by every adapter-request site with the chosen adapter's name and whether it is a
/// software one (`DeviceType::Cpu`, or obtained by the fallback retry): on a lane that requires a
/// GPU a software adapter counts as no adapter and panics, so a lost discrete GPU (Windows then
/// offers WARP) cannot let GPU tests pass on a rasterizer.
pub fn assert_hardware_adapter(adapter_name: &str, software: bool) {
    software_gate(adapter_name, software, gpu_required());
}

fn software_gate(adapter_name: &str, software: bool, required: bool) {
    if software && required {
        panic!(
            "software GPU adapter {adapter_name:?} on a lane that requires one \
             ({REQUIRE_GPU_ENV}=1): the discrete GPU is gone and the OS fell back to a \
             rasterizer — check the driver / that the process has a GPU session, or unset \
             {REQUIRE_GPU_ENV} on a GPU-less box"
        );
    }
}

/// Whether this process's lane forbids creating a GPU device.
pub fn gpu_forbidden() -> bool {
    parse_setting(
        FORBID_GPU_ENV,
        std::env::var(FORBID_GPU_ENV).ok().as_deref(),
    )
}

/// Called first by every device-creation site: panics when the lane forbids a GPU.
pub fn assert_gpu_allowed() {
    forbid(gpu_forbidden());
}

fn forbid(forbidden: bool) {
    if forbidden {
        panic!(
            "a GPU device was requested on a lane that forbids one ({FORBID_GPU_ENV}=1): name the \
             test `needs_gpu_*` so the `no-gpu` nextest profile excludes it"
        );
    }
}

fn gate(no_adapter: bool, required: bool) -> bool {
    if no_adapter && required {
        panic!(
            "no GPU adapter (L2003) on a lane that requires one ({REQUIRE_GPU_ENV}=1): this run \
             gates images and must not skip — check the driver / that the process has a GPU \
             session, or unset {REQUIRE_GPU_ENV} on a GPU-less box"
        );
    }
    no_adapter
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "LUDION_REQUIRE_GPU")]
    fn a_required_lane_panics_on_a_missing_adapter() {
        gate(true, true);
    }

    #[test]
    fn an_optional_lane_skips_and_an_adapter_never_trips() {
        assert!(gate(true, false));
        assert!(!gate(false, true));
        assert!(!gate(false, false));
    }

    #[test]
    fn the_setting_parses_one_and_zero() {
        assert!(!require_gpu_setting(None));
        assert!(!require_gpu_setting(Some("")));
        assert!(!require_gpu_setting(Some("0")));
        assert!(require_gpu_setting(Some("1")));
    }

    #[test]
    #[should_panic(expected = "Microsoft Basic Render Driver")]
    fn a_required_lane_panics_on_a_software_adapter() {
        software_gate("Microsoft Basic Render Driver", true, true);
    }

    #[test]
    fn a_software_adapter_is_fine_off_the_required_lane_and_hardware_never_trips() {
        software_gate("WARP", true, false);
        software_gate("RTX 3090", false, true);
    }

    #[test]
    #[should_panic(expected = "LUDION_FORBID_GPU")]
    fn a_forbidding_lane_panics_on_device_creation() {
        forbid(true);
    }

    #[test]
    fn an_allowing_lane_never_trips() {
        forbid(false);
    }

    #[test]
    #[should_panic(expected = "LUDION_FORBID_GPU")]
    fn an_unknown_forbid_setting_panics() {
        parse_setting(FORBID_GPU_ENV, Some("yes"));
    }

    #[test]
    #[should_panic(expected = "LUDION_REQUIRE_GPU")]
    fn an_unknown_setting_panics() {
        require_gpu_setting(Some("yes"));
    }
}
