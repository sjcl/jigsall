use bevy::prelude::*;
use bevy::sprite::ColorMaterial;
use crate::components::*;
use crate::resources::*;
use crate::puzzle::*;
use crate::jigsaw_shapes::JigsawShapeGenerator;



fn spawn_grid_reference(
    commands: &mut Commands,
    puzzle_config: &PuzzleConfig,
    puzzle_image: Option<&Res<PuzzleImage>>,
) {
    // 選択された画像を半透明で表示
    if !puzzle_config.image_path.is_empty() {
        if let Some(puzzle_img) = puzzle_image {
            println!("📖 Spawning grid reference with existing image handle: {:?}", puzzle_img.handle.id());
            
            // 既に読み込み済みのImageハンドルを使用
            let texture_handle = puzzle_img.handle.clone();
            
            // Use the same size calculation as pieces to ensure alignment
            let custom_size = Some(puzzle_img.size);
            
            commands.spawn((
                Sprite {
                    color: Color::srgb(1.0, 1.0, 1.0).with_alpha(0.3), // 半透明
                    image: texture_handle,
                    custom_size, // Match the size used for piece calculations
                    ..default()
                },
                Transform::from_translation(Vec3::new(0.0, 0.0, -10.0)), // 背景に配置
                // 参照画像としてマーク
                GridReference,
            ));
            
            println!("✅ Grid reference spawned with size: ({:.1}, {:.1})", puzzle_img.size.x, puzzle_img.size.y);
        } else {
            println!("⚠️ Cannot spawn grid reference: PuzzleImage resource not available");
        }
    }
}

/// パズルをリセットする（既存のピース、グリッド背景、画像設定を削除）
pub fn reset_puzzle(
    mut commands: Commands,
    mut game_data: ResMut<GameData>,
    mut puzzle_config: ResMut<PuzzleConfig>,
    puzzle_pieces: Query<Entity, With<PuzzlePiece>>,
    grid_references: Query<Entity, With<GridReference>>,
    mut input_state: ResMut<InputState>,
    mut piece_cache: ResMut<PieceSelectionCache>,
) {
    if !game_data.needs_reset {
        return;
    }
    
    println!("🔄 Resetting puzzle completely...");
    
    // すべてのパズルピースを削除
    for entity in puzzle_pieces.iter() {
        commands.entity(entity).despawn();
    }
    
    // グリッド背景画像も削除
    for entity in grid_references.iter() {
        commands.entity(entity).despawn();
    }
    
    // ゲーム状態をリセット
    game_data.puzzle_completed = false;
    game_data.puzzle_progress = 0.0;
    game_data.needs_reset = false;
    
    // 入力状態をリセット
    input_state.selected_piece = None;
    input_state.next_z_order = 1.0;
    input_state.selected_pieces.clear();
    input_state.selected_pieces_set.clear();
    input_state.multi_drag_offset.clear();
    input_state.last_selection_rect = None;
    input_state.cached_drag_entity = None;
    
    // パフォーマンスキャッシュをクリア
    piece_cache.all_pieces.clear();
    piece_cache.piece_positions.clear();
    piece_cache.piece_bounds.clear();
    piece_cache.need_refresh = true;
    
    // 画像設定を完全にクリア
    puzzle_config.image_path.clear();
    
    // PuzzleImageリソースを削除して再読み込みを強制
    commands.remove_resource::<crate::resources::PuzzleImage>();
    
    println!("✅ Puzzle reset completed (pieces, grid background, and image settings cleared)");
}

pub fn spawn_puzzle_pieces_progressive(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    puzzle_config: Res<PuzzleConfig>,
    puzzle_image: Option<Res<PuzzleImage>>,
    _game_state: Res<GameData>,
    existing_pieces: Query<&PuzzlePiece>,
    existing_grid_ref: Query<&GridReference>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut progress: ResMut<PieceGenerationProgress>,
    mut stroke_cache: ResMut<StrokeMeshCache>,
    mut piece_cache: ResMut<PieceSelectionCache>,
    mut perf_monitor: ResMut<PerformanceMonitor>,
    mut next_sub_state: ResMut<NextState<GameSubState>>,
) {
    let _span = info_span!("spawn_puzzle_pieces_progressive").entered();
    let start_time = perf_monitor.start_system_timing("spawn_puzzle_pieces_progressive");
    
    // システム実行のデバッグログ（スポーン中のみ）
    if progress.generation_phase == GenerationPhase::SpawningEntities {
        static mut SYSTEM_CALL_COUNT: usize = 0;
        unsafe {
            SYSTEM_CALL_COUNT += 1;
            println!("🔄 System call #{}: spawned={}, pending={}", 
                SYSTEM_CALL_COUNT, progress.pieces_created, progress.pending_pieces.len());
        }
    }
    
    // 生成中でない場合で、かつ既にピースがある場合は何もしない
    if !progress.is_generating && !existing_pieces.is_empty() {
        perf_monitor.end_system_timing("spawn_puzzle_pieces_progressive", start_time);
        return;
    }

    // PuzzleImageが準備されていない場合は待機
    let Some(ref puzzle_image) = puzzle_image else {
        println!("⚠️ PuzzleImage not ready yet, waiting for GameSetup to complete asset loading...");
        perf_monitor.end_system_timing("spawn_puzzle_pieces_progressive", start_time);
        return;
    };

    // 画像サイズが適切でない場合は待機
    if puzzle_image.size.x <= 10.0 || puzzle_image.size.y <= 10.0 {
        perf_monitor.end_system_timing("spawn_puzzle_pieces_progressive", start_time);
        return;
    }

    // グリッド背景を表示（まだない場合）
    if existing_grid_ref.is_empty() && !progress.is_generating {
        spawn_grid_reference(&mut commands, &puzzle_config, Some(puzzle_image));
    }

    // 生成を開始
    if !progress.is_generating {
        let (grid_width, grid_height) = puzzle_config.grid_size;
        let total_pieces = grid_width * grid_height;
        
        println!("🎮 Starting progressive puzzle generation: {} pieces", total_pieces);
        
        progress.is_generating = true;
        progress.total_pieces = total_pieces;
        progress.grid_size = puzzle_config.grid_size;
        progress.generation_phase = GenerationPhase::PreparingShapes;
        progress.current_piece = 0;
        progress.shapes_generated = 0;
        progress.pieces_created = 0;

        // 画像サイズとピースサイズを計算
        let display_width = puzzle_image.size.x;
        let display_height = puzzle_image.size.y;
        let _piece_width = display_width / grid_width as f32;
        let _piece_height = display_height / grid_height as f32;

        // 標準スレッドでバックグラウンド処理を実行（crossbeam channelを使用）
        let (sender, receiver) = crossbeam::channel::unbounded();
        
        std::thread::spawn(move || {
            println!("🧵 Background thread started for shape generation");
            
            // 画像サイズとピースサイズを計算
            let piece_width = display_width / grid_width as f32;
            let piece_height = display_height / grid_height as f32;

            // ジグソー形状ジェネレータを初期化
            let mut shape_generator = JigsawShapeGenerator::new(
                (piece_width, piece_height),
                (grid_width, grid_height),
            );

            // ジグソーテンプレートを先に生成
            if let Err(e) = shape_generator.generate_jigsaw_template() {
                println!("Failed to generate jigsaw template: {}", e);
                return;
            }

            // 全ての形状を生成する
            let _generated_count = 0;
            
            if let Err(e) = shape_generator.generate_all_shapes() {
                println!("Failed to generate all shapes: {}", e);
                return;
            }

            // 配置位置を生成
            let placement_positions = generate_placement_grid(
                grid_width, 
                grid_height, 
                piece_width, 
                piece_height,
                display_width,
                display_height
            );

            // 結果を送信
            let result = ShapeGenerationResult {
                shape_generator,
                placement_positions,
                grid_size: (grid_width, grid_height),
                total_pieces,
            };

            if let Err(e) = sender.send(result) {
                println!("Failed to send shape generation result: {}", e);
            }
        });

        // レシーバーを保存
        progress.bg_thread_receiver = Some(receiver);
        
        return;
    }

    // バックグラウンド処理の結果をチェック
    if let Some(ref receiver) = progress.bg_thread_receiver {
        if let Ok(result) = receiver.try_recv() {
            println!("🧵 Background shape generation completed");
            
            // 第二段階: ピース作成の準備
            progress.generation_phase = GenerationPhase::CreatingPieces;
            progress.placement_positions = result.placement_positions.clone();
            
            // 第二段階のバックグラウンド処理: 実際のピース作成
            let placement_positions = result.placement_positions;
            let shape_generator = result.shape_generator;
            let (grid_width, grid_height) = result.grid_size;
            let total_pieces = result.total_pieces;
            let display_width = puzzle_image.size.x;
            let display_height = puzzle_image.size.y;
            let image_handle = puzzle_image.handle.clone();
            
            let (piece_sender, piece_receiver) = crossbeam::channel::unbounded();
            
            std::thread::spawn(move || {
                println!("🧵 Background thread started for piece creation");
                
                let result = create_all_pieces_sync(
                    shape_generator,
                    placement_positions,
                    grid_width,
                    grid_height,
                    total_pieces,
                    display_width,
                    display_height,
                    image_handle,
                );
                
                if let Err(e) = piece_sender.send(result) {
                    println!("Failed to send piece creation result: {}", e);
                }
            });
            
            progress.piece_thread_receiver = Some(piece_receiver);
            progress.bg_thread_receiver = None; // レシーバーを解放
        }
    }

    // ピース作成の結果をチェック
    if let Some(ref receiver) = progress.piece_thread_receiver {
        if let Ok(result) = receiver.try_recv() {
            println!("🧵 Background piece creation completed: {} pieces", result.pieces.len());
            
            // メインスレッドでの処理に移行
            progress.generation_phase = GenerationPhase::SpawningEntities;
            progress.pending_pieces = result.pieces;
            progress.pieces_created = 0;
            progress.pieces_spawned_this_frame = 0;
            
            progress.piece_thread_receiver = None; // レシーバーを解放
        }
    }

    // メインスレッドでのエンティティ生成
    if progress.generation_phase == GenerationPhase::SpawningEntities {
        let batch_size = 10; // 1フレームあたりのスポーン数
        let mut spawned_count = 0;
        
        while spawned_count < batch_size && !progress.pending_pieces.is_empty() {
            let piece_data = progress.pending_pieces.remove(0);
            
            // メッシュをアセットに追加
            let mesh_handle = meshes.add(piece_data.mesh);
            
            // マテリアルを作成
            let material = ColorMaterial {
                texture: Some(puzzle_image.handle.clone()),
                ..default()
            };
            let material_handle = materials.add(material);
            
            // ストロークメッシュをキャッシュに追加
            if let Some(stroke_mesh) = piece_data.stroke_mesh {
                let stroke_mesh_handle = meshes.add(stroke_mesh);
                stroke_cache.stroke_meshes.insert(piece_data.piece_shape.shape_hash.clone(), stroke_mesh_handle);
            }
            
            // エンティティを生成
            commands.spawn((
                Mesh2d(mesh_handle),
                MeshMaterial2d(material_handle),
                piece_data.transform,
                piece_data.piece_component,
                piece_data.piece_shape,
                PickablePiece {
                    drag_offset: Vec2::ZERO,
                },
                Draggable {
                    is_dragging: false,
                    drag_offset: Vec2::ZERO,
                },
            ));
            
            spawned_count += 1;
            progress.pieces_created += 1;
            progress.pieces_spawned_this_frame += 1;
            
            // パフォーマンスキャッシュをマーク（新しいピースが追加された）
            piece_cache.need_refresh = true;
        }
        
        // 進捗ログ（頻度制限）
        if progress.pieces_spawned_this_frame > 0 {
            static mut SPAWN_LOG_COUNT: usize = 0;
            unsafe {
                SPAWN_LOG_COUNT += 1;
                if SPAWN_LOG_COUNT % 10 == 0 {
                    println!("🎯 Spawned {} pieces (total: {}/{})", 
                        progress.pieces_spawned_this_frame, progress.pieces_created, progress.total_pieces);
                }
            }
        }
        
        // 全てのピースが生成完了したかチェック
        if progress.pending_pieces.is_empty() {
            println!("✅ All puzzle pieces spawned successfully: {} pieces", progress.pieces_created);
            
            // 生成完了
            progress.is_generating = false;
            progress.generation_phase = GenerationPhase::Completed;
            
            // リソースをクリーンアップ
            progress.placement_positions.clear();
            progress.bg_thread_receiver = None;
            progress.piece_thread_receiver = None;
            progress.progress_receiver = None;
            
            // GameSubStateをPlayingに遷移（パズル生成完了）
            next_sub_state.set(GameSubState::Playing);
            println!("🎮 Transitioned to Playing state - puzzle generation completed");
            
            // 大量パズルの場合: パフォーマンス確認
            if progress.total_pieces > 1000 {
                println!("🚀 Large puzzle generated: {} pieces", progress.total_pieces);
            }
        }
    }
    
    perf_monitor.end_system_timing("spawn_puzzle_pieces_progressive", start_time);
}

fn create_all_pieces_sync(
    shape_generator: JigsawShapeGenerator,
    placement_positions: Vec<Vec2>,
    grid_width: usize,
    grid_height: usize,
    total_pieces: usize,
    display_width: f32,
    display_height: f32,
    _image_handle: Handle<Image>,
) -> PieceCreationResult {
    use crate::jigsaw_shapes::clone_mesh_from_shape;
    use crate::components::*;
    use uuid::Uuid;
    use bevy::sprite::ColorMaterial;
    
    let mut pieces = Vec::with_capacity(total_pieces);
    let piece_width = display_width / grid_width as f32;
    let piece_height = display_height / grid_height as f32;
    
    println!("🧵 Starting piece creation loop for {} pieces", total_pieces);
    
    for piece_index in 0..total_pieces {
        let y = piece_index / grid_width;
        let x = piece_index % grid_width;
        
        if let Some(shape) = shape_generator.get_shape(x, y) {
            let piece_id = Uuid::new_v4();
            
            // 正しい位置を計算
            let correct_x = (x as f32 - (grid_width as f32 - 1.0) / 2.0) * piece_width;
            let correct_y = ((grid_height as f32 - 1.0) / 2.0 - y as f32) * piece_height;
            let correct_position = Vec2::new(correct_x, correct_y);
            
            // 開始位置を取得
            let start_position = if piece_index < placement_positions.len() {
                placement_positions[piece_index]
            } else {
                Vec2::new(0.0, 0.0)
            };
            
            let texture_coords = Vec4::new(
                x as f32 / grid_width as f32,
                y as f32 / grid_height as f32,
                (x + 1) as f32 / grid_width as f32,
                (y + 1) as f32 / grid_height as f32,
            );
            
            // 当たり判定用の境界
            let margin_ratio = 1.3;
            let half_width = (piece_width * margin_ratio) / 2.0;
            let half_height = (piece_height * margin_ratio) / 2.0;
            let collision_bounds = Rect::new(
                -half_width,
                -half_height,
                half_width,
                half_height
            );
            
            let piece_component = PuzzlePiece {
                id: piece_id,
                original_position: start_position,
                current_position: start_position,
                correct_position,
                texture_coords,
                is_placed: false,
                grid_x: x,
                grid_y: y,
                bounds: collision_bounds,
            };
            
            let z_offset = (y * grid_width + x) as f32 * 0.001;
            
            // メッシュをクローン（ここが重い処理だがバックグラウンドで実行）
            if piece_index < 5 {
                println!("🧵 Cloning mesh for piece {}", piece_index);
            }
            let mesh = clone_mesh_from_shape(shape);
            if piece_index < 5 {
                println!("🧵 Extracting shape data for piece {}", piece_index);
            }
            let piece_shape = extract_shape_data_from_jigsaw_shape(shape);
            
            // マテリアルハンドルを作成（Bevyアセットは非同期では作成できないため、ハンドルのみ）
            let material_handle = Handle::<ColorMaterial>::default(); // 後でメインスレッドで設定
            
            let transform = Transform::from_translation(start_position.extend(z_offset));
            
            pieces.push(PieceData {
                mesh,
                stroke_mesh: shape.stroke_mesh.clone(),
                piece_component,
                piece_shape,
                transform,
                material_handle, // 一時的な値
            });
        }
        
        // 進捗表示（より頻繁に）
        if piece_index % 100 == 0 {
            println!("🧵 Background piece creation progress: {}/{}", piece_index, total_pieces);
        }
    }
    
    println!("🧵 Piece creation completed: {} pieces", pieces.len());
    
    PieceCreationResult {
        pieces,
    }
}