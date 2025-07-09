use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

/// フォント設定を初期化する（日本語フォント対応）
pub fn setup_fonts(mut contexts: EguiContexts) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };
    
    // デフォルトの日本語フォント設定
    let mut fonts = egui::FontDefinitions::default();
    
    // Windowsの標準日本語フォントを追加
    #[cfg(target_os = "windows")]
    {
        // Windows標準の日本語フォント
        if let Ok(font_data) = std::fs::read("C:/Windows/Fonts/msgothic.ttc") {
            fonts.font_data.insert(
                "msgothic".to_owned(),
                egui::FontData::from_owned(font_data).into(),
            );
            
            fonts.families.entry(egui::FontFamily::Proportional).or_default()
                .insert(0, "msgothic".to_owned());
        } else if let Ok(font_data) = std::fs::read("C:/Windows/Fonts/meiryo.ttc") {
            fonts.font_data.insert(
                "meiryo".to_owned(),
                egui::FontData::from_owned(font_data).into(),
            );
            
            fonts.families.entry(egui::FontFamily::Proportional).or_default()
                .insert(0, "meiryo".to_owned());
        }
    }
    
    // フォールバック: 英語UIに変更
    ctx.set_fonts(fonts);
}