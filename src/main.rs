// Hide the console window in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod crypto;
mod db;
mod generator;
mod mailer;
mod protect;
mod render;
mod theme;
mod totp;
mod vault;
mod widgets;

fn app_icon() -> egui::IconData {
    // Procedurally drawn rounded gradient tile with a keyhole.
    let n = 64u32;
    let mut rgba = Vec::with_capacity((n * n * 4) as usize);
    let (a, b) = ([0x8bu8, 0x7b, 0xff], [0x4fu8, 0xd1, 0xc5]);
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let r = 16.0f32;
            let cx = fx.clamp(r, n as f32 - r);
            let cy = fy.clamp(r, n as f32 - r);
            let inside = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt() <= r;
            let t = (fx + fy) / (2.0 * n as f32);
            let mut px = [0u8; 4];
            if inside {
                for i in 0..3 {
                    px[i] = (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t) as u8;
                }
                px[3] = 255;
                let (dx, dy) = (fx - 32.0, fy - 27.0);
                let hole = (dx * dx + dy * dy).sqrt() < 8.0 || (dx.abs() < 4.0 && fy > 27.0 && fy < 46.0);
                if hole {
                    px = [255, 255, 255, 255];
                }
            }
            rgba.extend_from_slice(&px);
        }
    }
    egui::IconData { rgba, width: n, height: n }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("SoftVault")
            .with_inner_size([1220.0, 780.0])
            .with_min_inner_size([980.0, 640.0])
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native("SoftVault", options, Box::new(|cc| Ok(Box::new(app::SoftVaultApp::new(cc)))))
}
