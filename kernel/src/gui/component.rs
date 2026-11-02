//! Native UI Component & Interactive Hit-Testing
//!
//! Models interactive touchscreen/pointer target cards and clickable widgets.

/// Interactive application surface card.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct UiCard {
    pub id: u32,
    pub title: &'static str,
    pub subtitle: &'static str,
    pub badge: &'static str,
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub accent: (u8, u8, u8),
    pub action_intent: &'static str,
}

impl UiCard {
    /// Returns true if the pointer coordinates fall within this card's interactive bounding box.
    pub fn hit_test(&self, px: u32, py: u32) -> bool {
        let x = px as usize;
        let y = py as usize;
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// Interactive Permission Request Modal Dialog with Capacitive Touch Action Buttons
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PermissionModal {
    pub request_id: u64,
    pub app_name: &'static str,
    pub permission_title: &'static str,
    pub permission_desc: &'static str,
    pub modal_x: usize,
    pub modal_y: usize,
    pub modal_w: usize,
    pub modal_h: usize,
    pub allow_btn_x: usize,
    pub allow_btn_y: usize,
    pub allow_btn_w: usize,
    pub allow_btn_h: usize,
    pub deny_btn_x: usize,
    pub deny_btn_y: usize,
    pub deny_btn_w: usize,
    pub deny_btn_h: usize,
}

impl PermissionModal {
    pub const fn new(
        request_id: u64,
        app_name: &'static str,
        permission_title: &'static str,
        permission_desc: &'static str,
        phone_x: usize,
        phone_y: usize,
        phone_w: usize,
    ) -> Self {
        let modal_x = phone_x + 24;
        let modal_y = phone_y + 200;
        let modal_w = phone_w - 48;
        let modal_h = 240;

        let btn_w = (modal_w - 48) / 2;
        let btn_h = 44;
        let btn_y = modal_y + modal_h - btn_h - 20;

        let allow_btn_x = modal_x + 16;
        let deny_btn_x = allow_btn_x + btn_w + 16;

        Self {
            request_id,
            app_name,
            permission_title,
            permission_desc,
            modal_x,
            modal_y,
            modal_w,
            modal_h,
            allow_btn_x,
            allow_btn_y: btn_y,
            allow_btn_w: btn_w,
            allow_btn_h: btn_h,
            deny_btn_x,
            deny_btn_y: btn_y,
            deny_btn_w: btn_w,
            deny_btn_h: btn_h,
        }
    }

    /// Checks if a pointer coordinate hits the modal backdrop/dialog box.
    pub fn hit_test_modal(&self, px: u32, py: u32) -> bool {
        let x = px as usize;
        let y = py as usize;
        x >= self.modal_x && x < self.modal_x + self.modal_w && y >= self.modal_y && y < self.modal_y + self.modal_h
    }

    /// Checks if a pointer coordinate hits the ALLOW button.
    pub fn hit_test_allow(&self, px: u32, py: u32) -> bool {
        let x = px as usize;
        let y = py as usize;
        x >= self.allow_btn_x && x < self.allow_btn_x + self.allow_btn_w && y >= self.allow_btn_y && y < self.allow_btn_y + self.allow_btn_h
    }

    /// Checks if a pointer coordinate hits the DENY button.
    pub fn hit_test_deny(&self, px: u32, py: u32) -> bool {
        let x = px as usize;
        let y = py as usize;
        x >= self.deny_btn_x && x < self.deny_btn_x + self.deny_btn_w && y >= self.deny_btn_y && y < self.deny_btn_y + self.deny_btn_h
    }
}

/// Interactive Sovereign Share Sheet Drawer Dialog with App Target Targets
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ShareSheetDrawer {
    pub tx_id: u64,
    pub mime_type: &'static str,
    pub uri_preview: &'static str,
    pub drawer_x: usize,
    pub drawer_y: usize,
    pub drawer_w: usize,
    pub drawer_h: usize,
    pub target1_x: usize,
    pub target1_y: usize,
    pub target1_w: usize,
    pub target1_h: usize,
    pub target2_x: usize,
    pub target2_y: usize,
    pub target2_w: usize,
    pub target2_h: usize,
    pub target3_x: usize,
    pub target3_y: usize,
    pub target3_w: usize,
    pub target3_h: usize,
    pub target4_x: usize,
    pub target4_y: usize,
    pub target4_w: usize,
    pub target4_h: usize,
}

impl ShareSheetDrawer {
    pub const fn new(
        tx_id: u64,
        mime_type: &'static str,
        uri_preview: &'static str,
        phone_x: usize,
        phone_y: usize,
        phone_w: usize,
        phone_h: usize,
    ) -> Self {
        let drawer_w = phone_w - 32;
        let drawer_h = 280;
        let drawer_x = phone_x + 16;
        let drawer_y = phone_y + phone_h - drawer_h - 16;

        let col_w = (drawer_w - 48) / 2;
        let row_h = 56;

        let col1_x = drawer_x + 16;
        let col2_x = col1_x + col_w + 16;

        let row1_y = drawer_y + 100;
        let row2_y = row1_y + row_h + 16;

        Self {
            tx_id,
            mime_type,
            uri_preview,
            drawer_x,
            drawer_y,
            drawer_w,
            drawer_h,
            // Target 1: Messages
            target1_x: col1_x,
            target1_y: row1_y,
            target1_w: col_w,
            target1_h: row_h,
            // Target 2: unbound grid slot (kept so slots 3 and 4 stay in place)
            target2_x: col2_x,
            target2_y: row1_y,
            target2_w: col_w,
            target2_h: row_h,
            // Target 3: Vault
            target3_x: col1_x,
            target3_y: row2_y,
            target3_w: col_w,
            target3_h: row_h,
            // Target 4: Browser
            target4_x: col2_x,
            target4_y: row2_y,
            target4_w: col_w,
            target4_h: row_h,
        }
    }

    pub fn hit_test_drawer(&self, px: u32, py: u32) -> bool {
        let x = px as usize;
        let y = py as usize;
        x >= self.drawer_x && x < self.drawer_x + self.drawer_w && y >= self.drawer_y && y < self.drawer_y + self.drawer_h
    }

    pub fn hit_test_target1(&self, px: u32, py: u32) -> bool {
        let x = px as usize;
        let y = py as usize;
        x >= self.target1_x && x < self.target1_x + self.target1_w && y >= self.target1_y && y < self.target1_y + self.target1_h
    }

    pub fn hit_test_target2(&self, px: u32, py: u32) -> bool {
        let x = px as usize;
        let y = py as usize;
        x >= self.target2_x && x < self.target2_x + self.target2_w && y >= self.target2_y && y < self.target2_y + self.target2_h
    }

    pub fn hit_test_target3(&self, px: u32, py: u32) -> bool {
        let x = px as usize;
        let y = py as usize;
        x >= self.target3_x && x < self.target3_x + self.target3_w && y >= self.target3_y && y < self.target3_y + self.target3_h
    }

    pub fn hit_test_target4(&self, px: u32, py: u32) -> bool {
        let x = px as usize;
        let y = py as usize;
        x >= self.target4_x && x < self.target4_x + self.target4_w && y >= self.target4_y && y < self.target4_y + self.target4_h
    }
}
