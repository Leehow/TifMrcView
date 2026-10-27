[English](README.md) | [简体中文](README.zh-CN.md) | [Deutsch](README.de.md) | [日本語](README.ja.md) | [Español](README.es.md) | **Français** | [Русский](README.ru.md)

# TifMrcView

Une visionneuse petite, rapide et pratique pour les images de microscopie et de microscopie électronique : TIFF / OME-TIFF / ImageJ, MRC pour la cryo-EM et la cryo-ET, Zeiss CZI et SmartSEM. Tracez des lignes, des rectangles et des cercles avec des mesures en unités physiques, passez des vues XY / XZ / YZ, et visualisez la fluorescence multicanal avec activation et couleur par canal. Le programme d'installation ne pèse que quelques Mo et ouvre pourtant instantanément des piles de plusieurs Go. Fonctionne sous macOS, Windows et Linux, et s'étend avec des plugins. Développé avec Rust + Tauri 2.

[![Download](https://img.shields.io/badge/download-tifmrcview.com-2f6fed)](https://tifmrcview.com/tifmrcview/#download)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey)
[![GitHub stars](https://img.shields.io/github/stars/Leehow/TifMrcView?style=social)](https://github.com/Leehow/TifMrcView)

**[Télécharger](https://tifmrcview.com/tifmrcview/#download)** · **[Marché de plugins](https://tifmrcview.com/tifmrcview/plugins/)** · [Site web](https://tifmrcview.com/tifmrcview/)

> Si TifMrcView vous est utile, merci de lui donner une ⭐ sur GitHub : cela aide vraiment.

## Captures d'écran

<table>
  <tr>
    <td width="50%"><a href="site/shots/mrc-tomogram-line.webp"><img src="site/shots/mrc-tomogram-line-1200.webp" alt="Tomogramme cryo-ET au format MRC avec mesure de longueur"></a><br><sub>Tomogramme cryo-ET (MRC) avec mesure de longueur et outils d'annotation.</sub></td>
    <td width="50%"><a href="site/shots/mrc-xz-view-box.webp"><img src="site/shots/mrc-xz-view-box-1200.webp" alt="Vue latérale XZ avec un rectangle"></a><br><sub>Vue latérale XZ avec un rectangle indiquant largeur × hauteur.</sub></td>
  </tr>
  <tr>
    <td width="50%"><a href="site/shots/czi-airyscan-measure.webp"><img src="site/shots/czi-airyscan-measure-1200.webp" alt="Pile Z Airyscan Zeiss CZI avec mesure"></a><br><sub>Pile Z Airyscan Zeiss CZI avec mesure et barre d'échelle.</sub></td>
    <td width="50%"><a href="site/shots/tiff-multichannel.webp"><img src="site/shots/tiff-multichannel-1200.webp" alt="TIFF de fluorescence à 3 canaux"></a><br><sub>TIFF de fluorescence à 3 canaux avec activation par canal.</sub></td>
  </tr>
</table>

## Fonctionnalités

- Formats pris en charge : TIFF (multipage, bandes/tuiles, compression LZW/Deflate/PackBits/JPEG, niveaux de gris de 1 à 7 bits comme le binaire 1 bit et le 4 bits, ainsi que les entiers et flottants 8/16/32 bits ; les profondeurs inhabituelles comme 12 bits donnent un message d'erreur explicite), OME-TIFF (lit les dimensions C/Z/T, les noms et couleurs de canaux, la taille de pixel), hyperpiles ImageJ (y compris les fichiers contigus de plus de 4 Go), TIFF à palette (comme les images SEM Zeiss), MRC/MRC2014 (modes 0/1/2/3/4/6/12/101, `.mrc` `.mrcs` `.rec` `.st` `.map` ; pour les fichiers de mode 0 sans marqueur IMOD, le minimum/maximum de l'en-tête sert à déterminer s'il s'agit d'octets non signés) et, avec le plugin Zeiss, Zeiss CZI (LSM / Airyscan, etc. enregistrés par ZEN : sous-blocs non compressés, Zstd et JPEG, niveaux de gris et couleur 8/16/32 bits, dimensions C/Z/T, mosaïques en tuiles, noms et couleurs de canaux, taille de pixel). `.rec` `.st` `.map` et `.czi` n'apparaissent que dans « Ouvrir avec » et ne sont pas définis comme application par défaut
- Ouvre aussi PNG / JPEG / BMP / GIF / WebP
- Curseur Z et lecture ; un curseur T supplémentaire quand il y a une dimension temporelle
- Composition couleur multicanal : la case à cocher affiche/masque un canal, un clic sur le nom règle le contraste de ce canal, un clic sur le point change sa couleur
- Luminosité / contraste / gamma ; glissez sur l'histogramme pour choisir directement la plage affichée ; contraste automatique par coupe par défaut (saturation de 0,1 %)
- Le bouton ⓘ de la barre d'outils affiche les informations de l'image : format, taille, taille de pixel et paramètres d'acquisition lus par les plugins (recherchables et copiables). Le plugin Zeiss lit tous les paramètres d'une image SEM Zeiss SmartSEM (tension, distance de travail, grossissement, détecteur, position de la platine, vide, …) et affiche une barre d'échelle d'après la taille de pixel calibrée, et dans un CZI le microscope, l'objectif, les lasers, les détecteurs, le pinhole, le balayage et les paramètres de traitement Airyscan
- Une barre d'aperçu à droite (repliable) liste les images du même dossier avec nom de fichier, format et taille
- Barre d'icônes en haut (noms au survol) : enregistrer l'image, annuler, sélectionner, déplacer, mesurer, crayon (la flèche à côté ouvre un menu : libre / ligne / flèche / rectangle / cercle), texte, recadrer, barre d'échelle, luminosité/contraste
- Les mesures affichent les longueurs réelles d'après la taille de pixel ; les rectangles et cercles affichent toujours largeur × hauteur sur l'image (le diamètre pour un cercle parfait)
- Outil de sélection (par défaut) : glissez dans le vide pour déplacer l'image ; cliquez sur une annotation pour la déplacer, glissez les points du contour pour la redimensionner, glissez les extrémités d'une ligne de mesure pour la remesurer ; Suppr supprime ; la couleur et l'épaisseur du trait sont modifiables
- Les annotations sont enregistrées par coupe Z et sauvegardées automatiquement environ 2 secondes après le tracé dans `<nom du fichier>.annot.json` à côté de l'image, puis relues à la prochaine ouverture
- Recadrage : encadrez une zone puis double-cliquez ou appuyez sur Entrée pour ne voir que cette partie ; Échap ou Retour arrière l'annule
- Un modèle IMOD (`.mod` / `.fid`) de même nom est superposé automatiquement et peut être activé par objet
- Enregistrement de l'image : PNG à la résolution d'origine (après recadrage, uniquement la zone recadrée), avec les annotations, le modèle et la barre d'échelle visibles
- Les images avec Z peuvent basculer en haut entre les vues XY / XZ / YZ (les annotations tracées dans les vues latérales ne sont pas enregistrées)
- Glissez le bord intérieur de la barre d'outils de gauche et de la barre d'aperçu de droite pour changer leur largeur
- Molette de la souris pour zoomer, glisser pour déplacer, double-clic pour ajuster à la fenêtre ; la barre d'état affiche les coordonnées du pixel et sa valeur brute
- Glissez un fichier dans la fenêtre pour l'ouvrir ; un double-clic sur un autre fichier du même dossier l'ouvre dans la fenêtre existante, tandis qu'un fichier d'un autre dossier ouvre une nouvelle fenêtre
- Le bouton « Ouvrir le dossier » en haut de la barre d'aperçu affiche l'image actuelle dans le Finder / l'Explorateur
- Avec n'importe quel autre outil, un clic droit sur l'image revient à l'outil de sélection
- Une annotation peut n'apparaître que sur une coupe ou couvrir une plage de Z ; une fois sélectionnée, on modifie son Z en haut, et le curseur Z marque les coupes qui ont des annotations

## Plugins

Le menu « Plugins » de la barre d'outils permet d'activer/désactiver, d'installer et de désinstaller des plugins, et de vérifier les mises à jour. À part les langues, chaque plugin s'installe séparément : cliquez sur « Marché de plugins » pour ouvrir https://tifmrcview.com/tifmrcview/plugins/ dans le navigateur et télécharger un paquet de plugin (.zip), puis cliquez sur « Charger un plugin… » et choisissez-le. Installer une version plus récente du même plugin remplace simplement l'ancienne. Les sources des plugins se trouvent dans le dépôt sous `plugins/<id>/` (`plugin.json` + `index.js` ; actuellement `recorder`, `zeiss`, `lasx`, `nis`, `fluoview` et `imaris`) ; les plugins installés se trouvent dans un sous-dossier `<id>/` du dossier des plugins.

Installation en un clic (0.5 et plus) : cliquez sur **Installer** sur la fiche d'un plugin sur le marché, ou dans la liste « Marché de plugins » du menu Plugins (qui propose « Mettre à jour vers x.y.z » quand une nouvelle version existe). Le site transmet à l'application un lien de la forme `tifmrcview://install-plugin?id=<id>` (éventuellement `&version=<x.y.z>`) ; l'application affiche le nom et la description du plugin et n'installe rien tant que vous n'avez pas cliqué sur **Installer**. Seuls les plugins publiés sur le marché officiel (`plugins.json` à côté du flux de mises à jour) sont installés, téléchargés en HTTPS et vérifiés (taille et SHA-256) avant l'installation. Télécharger le zip et utiliser « Installer un plugin… » fonctionne toujours.

Plugins de format : un paquet de plugin peut aussi ajouter des formats de fichier et des lecteurs de métadonnées sous forme de module WebAssembly isolé, écrit par exemple en Rust avec le SDK de `plugins/sdk`. Chaque fabricant a un tel plugin (`zeiss` ouvre CZI et LSM et lit les paramètres SmartSEM) ; le programme ne contient aucun code de format propre à un fabricant : sans le plugin Zeiss, les fichiers .czi ne s'ouvrent pas et l'application indique quel plugin installer. Un module ne voit que le fichier ouvert, tourne avec des limites de mémoire et de temps et décrit chaque page comme une liste de morceaux du fichier que l'application décompresse et assemble elle-même, en parallèle. ABI, limites et exemple : [docs/format-plugins.md](docs/format-plugins.md).

Plugin d'enregistrement vidéo : dans le panneau « Outils », sous « Enregistrer une vidéo (MP4) », choisissez Z ou T, l'image de début et de fin, la fréquence d'images, la région (image entière / sélection / vue actuelle), la taille et la qualité. Le MP4 H.264 obtenu contient le contraste actuel, les couleurs des canaux, les annotations et la barre d'échelle.

La langue de l'interface est aussi un plugin intégré (`ui/plugins/i18n.js`) : il prend en charge 中文, English, Deutsch, 日本語, Español, Français et Русский, suit la langue du système par défaut et se change avec la liste déroulante de la barre d'outils. Il remplace les textes chinois connus de l'interface par la langue choisie, si bien que les autres plugins et messages sont traduits aussi ; pour ajouter un texte, ajoutez une ligne à `WORDS` (texte fixe) ou `PATTERNS` (texte contenant des nombres ou des noms de fichiers). Désactiver ce plugin ramène au chinois.

Écrire votre propre plugin : placez un fichier `.js` dans le dossier des plugins et cliquez sur « Recharger ». Pour le distribuer, empaquetez `plugin.json` (`id`, `name`, `version`, `description`, `main`, `apiVersion`, `minAppVersion`, et en option `author`, `i18n`, `details` ; voir le format dans `plugins/recorder/plugin.json`) avec le script dans un zip (fichiers à la racine du zip). Un exemple minimal :

```js
TifMrcView.register({
  id: "hello",
  name: "Hello plugin",
  version: "1.0.0",
  description: "Shows the current slice in the tools panel",
  activate(app) {
    const box = app.ui.addToolSection("Hello");
    app.on("slice", ({ z }) => (box.textContent = `Slice ${z + 1}`));
  },
});
```

`app` fournit : `state()` (fichier actuel, Z/T, taille, sélection, taille de pixel), `on("open" | "slice" | "render", fn)`, `setZ` / `setT` / `open(path)`, `renderRegion(region, {z, t, size})` (rend dans un canvas avec les réglages d'affichage actuels, annotations et barre d'échelle comprises), `ui.addToolSection` / `ui.addToolbarButton` / `ui.addToolbarSelect` / `ui.toast` / `ui.busy`, `dialog.save`, `fs.write`, `settings.get/set`, et `native(method, args, bytes)` pour appeler le côté Rust. La définition complète se trouve dans `ui/plugin-host.js`. Les plugins qui ont besoin de code natif (comme l'encodeur H.264 de l'enregistrement) implémentent `NativePlugin` dans `src-tauri/src/plugins/` ; la partie native est compilée dans le programme principal mais n'est activée qu'une fois un paquet de plugin de même id installé. Une feature Cargo détermine si l'enregistrement est compilé (`recorder` est activée par défaut ; `--no-default-features` la retire et réduit le programme d'installation d'environ 1 Mo).

Les plugins ont les mêmes permissions que le programme principal : n'installez que des plugins de confiance.

## Mises à jour automatiques

Quelques secondes après le démarrage, l'application vérifie discrètement `https://tifmrcview.com/tifmrcview/latest.json`. Lorsqu'une nouvelle version existe, une bannière apparaît en haut ; « Mettre à jour maintenant et redémarrer » télécharge, vérifie la signature, installe et redémarre. Les paquets de mise à jour sont signés avec une clé minisign de Tauri ; la clé publique figure dans `tauri.conf.json`.

## Raccourcis clavier

| Touche | Action |
| --- | --- |
| ↑ / ↓ | Fichier précédent / suivant |
| ← / →, PageUp / PageDown, `,` / `.` | Changer de Z |
| Début / Fin | Première / dernière coupe |
| Espace | Lire / mettre en pause Z |
| Ctrl + molette (⌘ + molette sous macOS), Maj + molette | Changer de Z |
| S / V / M / P / L / A / R / E / X / C | Sélectionner / déplacer / mesurer / crayon / ligne / flèche / rectangle / cercle / texte / recadrer |
| Maj (en traçant un rectangle ou un cercle) | Carré / cercle parfait |
| Maj (en traçant une ligne ou une flèche) | Aligner sur 45° |
| Suppr / Retour arrière | Supprimer l'annotation sélectionnée ; sans sélection, annuler le recadrage |
| Échap | Annuler l'annotation en cours ou venant d'être tracée, désélectionner, annuler le recadrage |
| Entrée, double-clic sur le cadre de recadrage | Recadrer |
| Ctrl/⌘ + Z | Annuler (tracé, mesure, déplacement, suppression et recadrage sont annulables) |
| Ctrl/⌘ + Maj + Z, Ctrl/⌘ + Y | Rétablir |
| Ctrl/⌘ + S | Enregistrer un PNG (à côté de l'image par défaut, ou dans le dossier d'enregistrement choisi dans le panneau Outils) |
| H | Afficher / masquer les annotations |
| I | Afficher / masquer le modèle IMOD |
| T | Ouvrir / replier le panneau « Outils » |
| B | Afficher / masquer la barre d'aperçu |
| F, double-clic (outil déplacer) | Ajuster à la fenêtre |
| 1 | Taille d'origine (1:1) |
| + / =, - | Zoom avant / arrière |
| Ctrl/⌘ + O | Ouvrir un fichier |

## Pourquoi c'est rapide

- Les fichiers sont lus par projection mémoire, et les bandes/tuiles d'une page sont décompressées en parallèle sur tous les cœurs du processeur
- Après l'ouverture d'un fichier, les autres coupes Z sont pré-décodées en mémoire en arrière-plan, de la plus proche à la plus éloignée de la coupe actuelle (environ un quart de la mémoire physique, 512 Mo–8 Go), de sorte que les images s'affichent immédiatement quand on fait glisser le curseur
- Seule la région visible est rendue, et seulement à la résolution de l'écran ; le RGBA 8 bits est transmis à l'interface en binaire, donc zoomer et déplacer une image de 4000×4000 reste fluide
- Les miniatures sont générées dans un pool de threads séparé pour ne pas concurrencer l'image courante sur le processeur
- L'interface est du HTML/JS pur, sans framework frontend ; les builds de release utilisent LTO et strip, et les programmes d'installation font environ 4–11 Mo (l'AppImage Linux embarque WebKit et pèse donc environ 84 Mo)

## Installation

Téléchargez depuis le site officiel **[tifmrcview.com/tifmrcview](https://tifmrcview.com/tifmrcview/#download)**. Les nouvelles versions sortent souvent et sont publiées uniquement là (pas de GitHub Releases) ; l'application se met aussi à jour depuis ce site :

- **macOS** (10.15 ou plus récent) : `TifMrcView_x.y.z_universal.dmg` (Intel et Apple silicon), à glisser dans « Applications ». Comme l'application n'a pas de signature de développeur Apple, le premier lancement est bloqué : allez dans « Réglages Système → Confidentialité et sécurité » et cliquez sur « Ouvrir quand même », ou exécutez `xattr -dr com.apple.quarantine /Applications/TifMrcView.app` dans un terminal. Au premier démarrage, TifMrcView se définit comme application par défaut pour .tif/.tiff/.mrc.
- **Windows** : `TifMrcView_x.y.z_x64-setup.exe`, double-cliquez pour installer (aucun droit administrateur requis). Le programme d'installation enregistre les associations .tif/.tiff/.mrc ; si vous aviez choisi manuellement un autre programme pour .tif (par exemple « Photos »), Windows interdit aux logiciels de le modifier en silence : cliquez sur « Définir par défaut » dans TifMrcView, ce qui ouvre la page des paramètres système, et choisissez-y TifMrcView.
- **Linux** : `.deb` pour Debian/Ubuntu, `.rpm` pour Fedora et assimilés, `.AppImage` pour les autres distributions. Au premier démarrage, l'application se définit par défaut via `xdg-mime`.

## Compiler depuis les sources

Il faut Rust (stable) et Node.js 18+. Sous Linux, il faut en plus :

```sh
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libxdo-dev libssl-dev
```

Puis :

```sh
npm install
npm run dev      # mode développement
npm run build    # génère le programme d'installation du système actuel, dans src-tauri/target/release/bundle/
```

Pour déboguer le décodage, utilisez le petit outil fourni ; il affiche les métadonnées, les pixels par canal et les temps de décodage/rendu :

```sh
cd src-tauri && cargo run --release --example probe -- /path/to/file.tif
```

## Publication

Les numéros de version sont générés automatiquement : le majeur.mineur de `tauri.conf.json` (actuellement 0.3) plus le nombre total de commits, par exemple `0.3.57` ; quand un tag `v1.2.3` est poussé, la version du tag est utilisée.

Chaque push sur `main` fait compiler des paquets par GitHub Actions sur macOS / Windows / Linux ; les programmes d'installation se trouvent dans les Artifacts de l'exécution. Il y a deux façons de publier sur le site de téléchargement :

- Pousser un tag commençant par `v` : `git tag v0.3.0 && git push origin v0.3.0` envoie sur le site de téléchargement et crée une GitHub Release ;
- ou lancer manuellement le build dans Actions en cochant « publier sur le site de téléchargement », ce qui publie avec le numéro de version automatique.

À la publication, la CI place les programmes d'installation et les paquets de mise à jour dans le répertoire `<version>/` du site de téléchargement, puis met à jour `latest.json` (pour la mise à jour automatique) et `downloads.json` (pour la page de téléchargement). La page de téléchargement et le marché de plugins (`site/`, `plugins/`) sont aussi déployés par leur propre workflow (`.github/workflows/site.yml`) dès qu'ils changent sur `main`. À configurer dans le dépôt sous Settings → Secrets and variables → Actions :

| Nom | Type | Description |
| --- | --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | Secret | Clé privée de signature des paquets de mise à jour (si elle est perdue, on ne peut plus pousser de mises à jour aux utilisateurs déjà installés) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Secret, optionnel | Mot de passe de cette clé, le cas échéant |
| `DEPLOY_SSH_KEY` | Secret | Clé privée SSH de l'utilisateur de déploiement du site de téléchargement |
| `DEPLOY_HOST` / `DEPLOY_USER` | Secret | Hôte et nom d'utilisateur du site de téléchargement |
| `UPDATE_BASE_URL` | Variable, optionnelle | Par défaut `https://tifmrcview.com/tifmrcview` |
| `DEPLOY_PATH` | Variable, optionnelle | Par défaut `/var/www/tifmrcview` |

## Structure du projet

| Chemin | Contenu |
| --- | --- |
| `src-tauri/src/formats/` | Décodeurs : `tiff.rs`, `mrc.rs`, `raster.rs` (PNG/JPEG/…), `mod.rs` |
| `src-tauri/src/` | `lib.rs` (commandes, gestion des fenêtres et fichiers), `render.rs`, `reslice.rs` (vues XZ/YZ), `image.rs`, `imod.rs` (modèles IMOD), `mem.rs`, `assoc.rs` (enregistrement comme application par défaut), `update.rs` |
| `src-tauri/src/plugins/` | Parties natives des plugins (`recorder.rs`, `mp4.rs`), paquets de plugins (`package.rs`) et hôte WebAssembly des plugins de format (`wasm.rs`, `recipe.rs`, `registry.rs`) |
| `src-tauri/examples/probe.rs` | L'outil de sondage du décodage |
| `ui/` | Frontend en HTML/JS pur : `index.html`, `app.js`, `style.css`, `plugin-host.js` ; plugin de langue intégré dans `ui/plugins/i18n.js` |
| `plugins/<id>/` | Sources des plugins installables (`recorder` et les plugins de format `zeiss`, `lasx`, `nis`, `fluoview` et `imaris`, un par fabricant, crates Rust compilées en WebAssembly) et le SDK des plugins (`sdk`) |
| `scripts/` | `pack-plugins.mjs` (construit les zip de plugins et l'index du marché), `make-release.mjs` |
| `site/` | Le site web et le marché de plugins, captures d'écran dans `site/shots/` |
| `packaging/linux/` | Fichiers de packaging Linux (type MIME de .mrc) |
| `.github/workflows/` | `build.yml` (build et publication), `site.yml` (déploiement du site et du marché de plugins) |

## Traductions

Le README est disponible en plusieurs langues (voir la ligne des langues tout en haut). **Le README anglais (`README.md`) fait foi** ; les autres fichiers conservent exactement les mêmes sections dans le même ordre. Quand vous modifiez `README.md`, mettez aussi à jour les traductions, ou ouvrez une issue si vous ne pouvez pas.

## Licence

Ce projet est open source sous [Apache License 2.0](LICENSE).
