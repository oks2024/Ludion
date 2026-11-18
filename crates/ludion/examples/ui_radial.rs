//! A tool wheel driven by a scripted stick sweep: `UiRadial::navigate` moves the selection by angle,
//! and `radial_hit` answers which item a cursor is over. Pure functions of the component, so no
//! engine is needed to try them (`ludion docs ui-radial`).
//!
//! ```text
//! cargo run -p ludion --example ui_radial
//! ```

use ludion::math::radial_hit;
use ludion::prelude::*;

fn main() {
    let item = |text: &str, enabled: bool| UiRadialItem {
        text: text.to_string(),
        enabled,
        ..UiRadialItem::default()
    };
    let mut wheel = UiRadial {
        anchor: Anchor::Center,
        items: vec![
            item("AXE", true),
            item("BOW", true),
            item("SHIELD", false),
            item("TORCH", true),
        ],
        ..UiRadial::default()
    };

    // Stick up, right, down (a disabled item: nothing changes), left.
    for stick in [[0.0, 1.0], [1.0, 0.0], [0.0, -1.0], [-1.0, 0.0]] {
        wheel.navigate(UiRadialNav::from_stick(stick));
        println!("stick {stick:?} -> selected {:?}", wheel.selected);
    }

    let activate = UiRadialNav {
        activate: true,
        ..UiRadialNav::default()
    };
    match wheel.navigate(activate) {
        Some(RadialEvent::Activated(i)) => println!("activated {}", wheel.items[i as usize].text),
        other => println!("{other:?}"),
    }

    // A cursor 150 px right of the centre of a 1920x1080 target is over the second item.
    let hit = radial_hit(&wheel, (1920, 1080), [960.0 + 150.0, 540.0]);
    println!("cursor over item {hit:?}");
    assert_eq!(hit, Some(1));
}
