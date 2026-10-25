# Fixtures audio de test

Ce dossier attend un fichier de référence pour le test
`tests/stt_transcription_test.rs` :

```
tests/fixtures/bonjour_ceci_est_un_test.wav
```

Format attendu : mono, 16 kHz, PCM 16 bits (exactement ce que produit
`audio::wav_writer::RecordingWavWriter` — le plus simple est d'enregistrer
avec l'application elle-même, `Ctrl+Shift+O`, puis de renommer/copier le
`.wav` obtenu ici).

Contenu attendu : quelqu'un prononçant clairement, en français, une phrase
contenant les mots **« bonjour »** et **« test »** (par exemple : *« Bonjour,
ceci est un test. »*).

Ce fichier n'est pas fourni avec le projet — il ne peut pas être généré
automatiquement (il faut une vraie voix) — et n'a pas vocation à être
commité dans le dépôt Git à long terme (données personnelles, poids inutile).
Sans lui, `transcribes_reference_french_audio` s'auto-ignore proprement
(voir le commentaire en tête de ce test).
