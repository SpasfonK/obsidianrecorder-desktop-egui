# ObsidianRECORDER Desktop — Mini-overlay vu-mètre (RMS live)

Dernier morceau du cahier des charges original : l'**overlay flottant avec
vu-mètre audio en temps réel** pendant l'enregistrement. Toutes les
itérations précédentes (socle audio, raccourci global, STT Whisper, export
Obsidian, réglages, System Tray, dashboard + lecture) restent inchangées —
voir les README précédents pour leur détail.

## 1. Ce qui est livré

| Exigence du cahier des charges | Statut |
|---|---|
| Mini-overlay flottant avec vu-mètre (waveform/RMS) | Livré : fenêtre secondaire egui, toujours au premier plan, visible uniquement pendant l'enregistrement |
| Transcription en direct dans l'overlay | Toujours hors périmètre — whisper.cpp n'a pas de décodage incrémental natif (raison détaillée dès l'Itération 2) |

## 2. Comment ça marche

- `audio::capture` calcule le niveau RMS de chaque trame entrante (avant
  ré-échantillonnage) et le publie dans un `Arc<AtomicU32>` partagé
  (stocké en bits, la bibliothèque standard n'ayant pas d'`AtomicF32`) —
  lu par `AudioCapture::level()`.
- `app.rs` lit ce niveau à chaque frame et affiche une petite fenêtre
  secondaire (`ctx.show_viewport_immediate`, le mécanisme multi-fenêtres
  natif d'egui) tant que l'enregistrement est actif : barre de progression
  simple, toujours au premier plan, sans bordure.
- Cette fenêtre n'a besoin d'aucune gestion d'état manuelle : une
  « immediate viewport » egui ne reste affichée que tant qu'on continue à
  l'appeler à chaque frame. Dès que l'enregistrement s'arrête, on cesse de
  l'appeler et elle disparaît d'elle-même.
- Le RMS brut d'une voix normale reste visuellement bas sur une échelle
  [0.0, 1.0] : un facteur ×4 (pragmatique, pas une vraie échelle en dB)
  rend la barre lisible. Affinable plus tard si besoin.

## 3. Point de vigilance (comme pour whisper-rs/bindgen en Itération 2)

Le multi-fenêtrage d'egui a un bug documenté et confirmé : combiner
`with_fullscreen` + `with_transparent` + `with_always_on_top` fait
planter l'application. Je n'utilise ici **ni** `with_fullscreen` **ni**
`with_transparent` — seulement `with_always_on_top` + `with_decorations(false)`
+ une taille fixe — donc cette combinaison précise n'est pas déclenchée.
Mais comme pour whisper-rs, c'est une API que je n'ai pas pu exercer
moi-même dans mon environnement (pas d'affichage, pas de Windows) : si le
build échoue ou si l'overlay se comporte mal à l'usage, dites-le-moi avec
le détail.

## 4. Ce qui reste volontairement hors périmètre

- **Position de l'overlay** : positionnement par défaut du système
  d'exploitation (pas de calcul explicite d'un coin d'écran) — un
  raffinement possible plus tard.
- **Forme waveform** (plutôt qu'une simple barre) : une barre de niveau
  reste la façon la plus simple et la plus fiable de représenter un RMS
  scalaire ; une vraie forme d'onde demanderait de conserver un historique
  d'échantillons, pas juste la dernière valeur.
- **Transcription en direct** : voir ligne 1 du tableau ci-dessus, inchangé
  depuis l'Itération 2.

Avec ceci, les quatre itérations du cahier des charges d'origine sont
couvertes (avec les adaptations assumées et documentées au fil du
projet — pivot Tauri→egui, pas d'installateur, etc.).

## 5. Tests

`compute_rms` (racine de la moyenne des carrés) est une fonction pure,
testée unitairement dans `audio/capture.rs` : silence → 0.0, signal
constant → son amplitude, buffer vide → 0.0. Le reste de la logique
(fenêtre secondaire, affichage) est de l'UI non testable sans dispositif
de rendu headless — même limite que pour le reste de l'interface, déjà
notée en Itération 4.

## 6. Construire et tester

```powershell
cargo build --release
cargo test --all
```

Aucune nouvelle dépendance Cargo (le multi-viewport fait partie d'`egui`,
déjà présent).
