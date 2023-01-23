use crate::commands::farm::dsn::PieceCache;
use async_trait::async_trait;
use std::error::Error;
use std::sync::Arc;
use subspace_core_primitives::{Piece, PieceIndex, PieceIndexHash};
use subspace_farmer_components::plotting::PieceGetter;
use subspace_networking::utils::multihash::ToMultihash;
use subspace_networking::utils::pieces::announce_single_piece_index_hash_with_backoff;
use subspace_networking::Node;
use tracing::debug;

pub(super) struct FarmerPieceGetter<PG, PC> {
    base_piece_getter: PG,
    piece_cache: Arc<tokio::sync::Mutex<PC>>,
    node: Node,
}

impl<PG, PC> FarmerPieceGetter<PG, PC> {
    pub(super) fn new(
        base_piece_getter: PG,
        piece_cache: Arc<tokio::sync::Mutex<PC>>,
        node: Node,
    ) -> Self {
        Self {
            base_piece_getter,
            piece_cache,
            node,
        }
    }
}

#[async_trait]
impl<PG, PC> PieceGetter for FarmerPieceGetter<PG, PC>
where
    PG: PieceGetter + Send + Sync,
    PC: PieceCache + Send + 'static,
{
    async fn get_piece(
        &self,
        piece_index: PieceIndex,
    ) -> Result<Option<Piece>, Box<dyn Error + Send + Sync + 'static>> {
        let piece_index_hash = PieceIndexHash::from_index(piece_index);
        let key = piece_index_hash.to_multihash().into();

        let maybe_should_store = {
            let piece_cache = self.piece_cache.lock().await;
            if let Some(piece) = piece_cache.get_piece(&key) {
                return Ok(Some(piece));
            }

            piece_cache.should_cache(&key)
        };

        let maybe_piece = self.base_piece_getter.get_piece(piece_index).await?;

        if let Some(piece) = &maybe_piece {
            if maybe_should_store {
                let mut piece_cache = self.piece_cache.lock().await;
                if piece_cache.should_cache(&key) && piece_cache.get_piece(&key).is_none() {
                    piece_cache.add_piece(key, piece.clone());
                    if let Err(error) =
                        announce_single_piece_index_hash_with_backoff(piece_index_hash, &self.node)
                            .await
                    {
                        debug!(
                            ?error,
                            ?piece_index_hash,
                            "Announcing retrieved and cached piece index hash failed"
                        );
                    }
                }
            }
        }

        Ok(maybe_piece)
    }
}
