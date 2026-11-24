use player::{MediaTime, PlaybackSeed, Player};
use std::sync::Arc;

pub fn assert_player_contracts(player: &Player, seed: &Arc<PlaybackSeed>) {
    fn assert_send_sync<T: Send + Sync>() {}
    fn assert_send<T: Send>(_: T) {}

    assert_send_sync::<Player>();
    assert_send_sync::<PlaybackSeed>();
    assert_send(player.play(seed.clone()));
    assert_send(player.pause());
    assert_send(player.resume());
    assert_send(player.seek(MediaTime::ZERO));
    assert_send(player.reset());
    assert_send(player.set_next(Some(seed.clone())));
}
