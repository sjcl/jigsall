use bevy::prelude::*;
use crate::resources::*;



/// 画像読み込みシステムをセットアップ
pub fn setup_image_load_system(mut commands: Commands) {
    // crossbeam unboundedチャネルを作成
    let (tx_results, rx_results) = crossbeam::channel::unbounded();
    
    // Bevyシステム側のチャネルをリソースとして追加
    commands.insert_resource(ImageLoadChannels {
        rx_results,
    });
    
    commands.insert_resource(ImageLoadSender {
        tx_results,
    });
    
    println!("🔧 Image load system initialized with crossbeam channels");
}



/// 画像読み込み結果を処理（crossbeam-channel受信）
pub fn handle_image_load_results(
    image_channels: Res<ImageLoadChannels>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
) {
    // crossbeam-channelから直接try_recv
    while let Ok(result) = image_channels.rx_results.try_recv() {
        println!("📨 MAIN THREAD [{:?}]: Received image load result for: {}", std::thread::current().id(), result.virtual_key);
        
        match result.image {
            Ok(image) => {
                println!("✅ Thread-based image loading completed for: {}", result.virtual_key);
                
                // 画像のサイズを取得
                let image_size = image.size();
                let size_vec2 = Vec2::new(image_size.x as f32, image_size.y as f32);
                
                // Imageアセットとして登録
                let handle = images.add(image);
                
                // PuzzleImageリソースを作成
                commands.insert_resource(PuzzleImage {
                    handle: handle.clone(),
                    size: size_vec2,
                });
                
                println!("📝 Created PuzzleImage with handle: {:?}, size: {:?}", handle.id(), size_vec2);
            }
            Err(e) => {
                println!("❌ Thread-based image loading failed for {}: {}", result.virtual_key, e);
            }
        }
    }
}

