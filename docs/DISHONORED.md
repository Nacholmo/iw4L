# Dishonored mode

Press **K** in a match you host and Corvo takes over the local soldier:
Dishonored's walk, sprint, crouch, slide, mantle, lean, swim, ladders and
Blink, his sword, the assassination and the drop assassination, with his arms,
Blink effects and sounds. Press K again for the
soldier. Everything Dishonored is read at startup from your own install; none
of it ships here. The motion and the install readers are
[sinhonor](https://github.com/Nacholmo/sinhonor)'s crates, a git dependency
pinned by tag in `Cargo.toml`; its tests run there, against your install.

| | keyboard / mouse | controller |
|---|---|---|
| toggle | K, or `dishonored on\|off\|status` | both sticks in |
| move, look, jump, crouch, sprint | your MW2 binds | your MW2 layout |
| Blink: hold to aim, release to go | ADS (right mouse) or F | ADS trigger |
| sword, assassinate, drop assassination | fire (left mouse) | fire trigger |
| Blink tier I / II | 1 / 2 | D-pad down / up |
| lean | Q / E | bumpers |
| slow walk | Alt | |

The install is found through `IW4L_DISHONORED`, then `DISHONORED_DIR`, then the
Steam libraries. `IW4L_DISHONORED_DIFFICULTY=easy|normal|hard|veryhard` picks
Corvo's attribute set. One Unreal unit is a centimetre, so Corvo keeps his
size: 1.75 m tall, 4 m/s running, a 2.1 m jump.

## Sword and takedowns

The attack button swings the sword, assassinates a player who hasn't noticed
you, and, while you fall onto someone, drops on them. How each move behaves is
sinhonor's (its README, and `NOTES.md` §5b, §5c and §5d); this fork only says
who the characters are and what happens to them:

- **Noticed or not.** A bot has noticed you while it sees you or is fighting
  you. A human has noticed you while facing you (within 60°) with nothing solid
  between. Players who have noticed you take sword blows; the rest can be
  assassinated, unless they are sprinting. Drop assassinations don't care.
- **Damage.** A blow takes the same share of an MW2 player's health as of a
  Dishonored character's, so the same number of blows kills, hardcore
  included. A killing blow, an assassination and a drop assassination kill
  outright.
- **In the match** every hit is a melee hit from you with the weapon you hold:
  `MOD_MELEE` through the game's damage callbacks, so the kill feed, score,
  killcam and blood all work. The host lands it on the next tick.
- **Paired kills** turn the victim and set them in front of Corvo where the
  kill animation needs them, before they die. A plain kill (no room, or
  falling) leaves them where they stand.
- The view follows the kill animation's camera, the swings and kills play
  their own sounds, and a blade that hits a wall shakes the view. Under the
  crosshair, the HUD shows **Assassinate** or **Drop assassination** with the
  key bound to fire.

| piece | where |
|---|---|
| mode, input, effect and sound triggers, hits and kills | `crates/render_anim/src/dishonored/mod.rs` |
| MW2 players as characters: awareness, health, facing | `…/dishonored/pawns.rs` |
| sweeps on PMove's collision, unit and axis conversion | `…/dishonored/world.rs` |
| arms and sword, Edge animations, CPU skinning | `…/dishonored/hands.rs` |
| Cascade particles as sprite batches | `…/dishonored/fx.rs` |
| Wwise cues decoded to PCM, footstep pacing | `…/dishonored/sound.rs` |
| arms, sprites and Blink vignette on the GPU | `crates/render_gpu/src/drawsurf/dishonored.rs` |
| PMove handed to an outside controller | `sim::SimWorld::{set_external_motion, with_player_clip}` |
| blows landed as melee damage, the look turned with Corvo | `sim::SimWorld::{strike, turn_view}`, `crates/sim/src/external_strike.rs` |
| who each bot has noticed | `frame::BotNotice`, written by `crates/bots/src/plugin.rs` |
| the attack prompt | `crates/console/src/dishonored.rs` |
| decoded clips as 2D one-shots | `audio::AudioRuntime::play_external` |
| camera, hidden weapon and reticle, zeroed commands | `view_kick.rs`, `fpv_present.rs`, `reticle.rs`, `net/src/client/runtime.rs` |

Like the skate mode it follows, it moves the player on the local authority, so
a client joined to someone else's match cannot turn it on. Other players see
the soldier's body glide without leg animation, and the kills without either
side's animation: the victim falls with an MW2 death. Bodies are MW2's fixed
box whatever the stance, so a crouched or prone player is struck and dropped on
as if standing. Bots that are held (`bot hold on`) count as humans for
noticing you. Dishonored's fall damage is not applied, and Blink's camera
motion blur is only the vignette.
