// The C ABI of a Ludion game's static library: the functions `ludion::ffi` exports, plus the one
// symbol the game exports itself. Kept in step with `crates/ludion/src/ffi.rs` by scripts/lists-in-step.mjs.
#ifndef LUDION_H
#define LUDION_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// The opaque handle a running game is held by.
typedef struct LudionEmbed LudionEmbed;

// surface_kind
#define LUDION_SURFACE_NONE 0
#define LUDION_SURFACE_UIKIT 1

// touch phases
#define LUDION_TOUCH_BEGAN 0
#define LUDION_TOUCH_MOVED 1
#define LUDION_TOUCH_ENDED 2
#define LUDION_TOUCH_CANCELLED 3

// ludion_touch results: MAPPED went through the layout; RESUMED woke a suspended engine and was
// swallowed (hide the tap-to-resume view); REFUSED is a null handle, an unknown phase, or no layout.
#define LUDION_TOUCH_MAPPED 0
#define LUDION_TOUCH_RESUMED 1
#define LUDION_TOUCH_REFUSED -1

// Strings are NUL-terminated UTF-8. Field order is the ABI.
typedef struct {
    const char *content_root;   // where content is read from
    const char *save_root;      // where the player profile is written
    uint32_t surface_kind;      // LUDION_SURFACE_*
    void *view;                 // the UIView* for LUDION_SURFACE_UIKIT
    uint32_t width, height;     // the surface's size in physical pixels
    bool paused;                // start the engine paused
    uint32_t safe_x, safe_y, safe_width, safe_height;  // the safe area in pixels; all zero = whole surface
    const char *quality;        // the shipped render quality, below LUDION_QUALITY; NULL or "" = none
} LudionStartParams;

typedef struct {
    uint32_t kind;              // 0 left stick, 1 right stick, 2 button
    float x, y, width, height;  // surface pixels, in the current safe rect
    const char *label;          // NULL for a stick; owned by the handle
} LudionTouchControl;

// The game's own symbol: starts the game, or returns NULL after printing why to stderr.
LudionEmbed *ludion_game_start(const LudionStartParams *params);

// Button indices follow GamepadButton's order (South 0 ... DpadRight 13); axis indices GamepadAxis's
// (LeftStickX 0 ... RightTrigger 5). Every call accepts a NULL handle.
bool ludion_pad_button(LudionEmbed *handle, uint32_t button, bool down);
bool ludion_pad_axis(LudionEmbed *handle, uint32_t axis, float value);
void ludion_resize(LudionEmbed *handle, uint32_t width, uint32_t height,
                   uint32_t safe_x, uint32_t safe_y, uint32_t safe_width, uint32_t safe_height);
bool ludion_suspend(LudionEmbed *handle);
bool ludion_resume(LudionEmbed *handle);
// True once the engine ended by itself (an argv mode finished, a wire engine.shutdown, a failed
// boot, a fatal render fault); the host should then ludion_stop (1 after a fault) and exit.
bool ludion_ended(LudionEmbed *handle);
int32_t ludion_touch(LudionEmbed *handle, uint64_t id, uint32_t phase, float x, float y);
uint32_t ludion_touch_control_count(LudionEmbed *handle);
bool ludion_touch_control(LudionEmbed *handle, uint32_t index, LudionTouchControl *out);
// Consumes the handle; 0 on success.
int32_t ludion_stop(LudionEmbed *handle);

#ifdef __cplusplus
}
#endif

#endif
