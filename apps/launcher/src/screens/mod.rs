//! One module per screen, so screens can be edited in parallel.
//!
//! WHY this exists: every renderer was a function inside `main.rs`, which meant two
//! people working on different screens collided on every save, and a single screen
//! change forced a 3,700-line file to be read to find context.
//!
//! Geometry and colour come from `crate::theme` and `crate::layout`. A screen must
//! not introduce its own hex literals or its own pixel arithmetic: the shell is
//! meant to adapt to whatever apps and options exist, and hardcoded values are
//! exactly how it stopped doing that.

pub mod activity;
pub mod app_drawer;
pub mod home;
pub mod live_widgets;
pub mod lock;
pub mod notifications;
pub mod power;
pub mod quick_controls;
pub mod system_space;

pub use activity::get_activity_color;
pub use app_drawer::get_app_drawer_color;
pub use home::get_home_color;
pub use live_widgets::get_live_widgets_color;
pub use lock::get_lock_screen_color;
pub use lock::{LOCK_DATE_TEXT_LEN, LOCK_MONTH_ABBR, LOCK_WEEKDAY_ABBR};
// WHY exported: the shell's touch handler hit-tests the lock screen's controls using
// the same geometry the screen draws with. That duplication is a known smell -- it is
// why these are shared rather than duplicated, so a layout change cannot leave
// the tap targets disagreeing with what is drawn.
pub use lock::{LOCK_BTN_CY, LOCK_BTN_L_CX, LOCK_BTN_R, LOCK_BTN_R_CX};
pub use notifications::get_notifications_color;
pub use power::get_power_menu_color;
pub use quick_controls::get_quick_controls_color;
pub use system_space::get_system_space_color;