(function () {
  var FLAGS = {
    zh: '<rect width="30" height="20" fill="#de2910"/><polygon fill="#ffde00" points="5,2.5 6.2,6.1 10,6.1 6.9,8.3 8.1,11.9 5,9.7 1.9,11.9 3.1,8.3 0,6.1 3.8,6.1" transform="translate(1,1)"/><circle cx="11" cy="2.5" r=".9" fill="#ffde00"/><circle cx="13" cy="4.5" r=".9" fill="#ffde00"/><circle cx="13" cy="7.5" r=".9" fill="#ffde00"/><circle cx="11" cy="9.5" r=".9" fill="#ffde00"/>',
    en: '<rect width="30" height="20" fill="#012169"/><path d="M0 0L30 20M30 0L0 20" stroke="#fff" stroke-width="4"/><path d="M0 0L30 20M30 0L0 20" stroke="#c8102e" stroke-width="1.6"/><path d="M15 0V20M0 10H30" stroke="#fff" stroke-width="6"/><path d="M15 0V20M0 10H30" stroke="#c8102e" stroke-width="3.4"/>',
    de: '<rect width="30" height="20" fill="#ffce00"/><rect width="30" height="13.34" fill="#dd0000"/><rect width="30" height="6.67" fill="#000"/>',
    ja: '<rect width="30" height="20" fill="#fff"/><circle cx="15" cy="10" r="6" fill="#bc002d"/>',
    es: '<rect width="30" height="20" fill="#aa151b"/><rect y="5" width="30" height="10" fill="#f1bf00"/>',
    fr: '<rect width="30" height="20" fill="#ef4135"/><rect width="20" height="20" fill="#fff"/><rect width="10" height="20" fill="#0055a4"/>',
    ru: '<rect width="30" height="20" fill="#d52b1e"/><rect width="30" height="13.34" fill="#0039a6"/><rect width="30" height="6.67" fill="#fff"/>'
  };
  var NAMES = { zh: '简体中文', en: 'English', de: 'Deutsch', ja: '日本語', es: 'Español', fr: 'Français', ru: 'Русский' };
  var HTML_LANG = { zh: 'zh-CN', en: 'en', de: 'de', ja: 'ja', es: 'es', fr: 'fr', ru: 'ru' };
  var XATTR = '<code>xattr -dr com.apple.quarantine /Applications/TifMrcView.app</code>';

  var T = {
    zh: {
      badge: '安装包只有 {range}，下载即用',
      title: 'TifMrcView · 快速的 TIFF / MRC 查看器',
      desc: 'TifMrcView：又小又快的 TIFF、OME-TIFF、MRC 图像查看器，支持 Z 层拉条、多通道合成、文件夹浏览。免费下载 macOS、Windows、Linux 版。',
      nav_features: '功能', nav_keys: '快捷键', nav_download: '下载',
      h1: '看 TIFF 和 MRC，<br>就该这么快',
      lead: '一个又小又快的显微图像查看器。双击 .tif / .tiff / .mrc 就能打开，拖动拉条翻 Z 层，多通道彩色合成，一个文件夹里的图左右切换。',
      cta: '免费下载', cta_os: '下载 {os} 版', all: '所有版本', version: '版本',
      mac_sub: 'macOS 11 及以上', win_sub: '无需管理员权限',
      mac_tip: '首次打开若被拦截：到「系统设置 → 隐私与安全性」点「仍要打开」，或在终端运行 ' + XATTR,
      win_tip: '如果 .tif 以前被别的程序占用，在 TifMrcView 里点「设为默认」，系统设置里选一下即可。',
      linux_tip: 'AppImage 自带运行库，下载后 <code>chmod +x</code> 即可运行。',
      dl_mac: '下载 .dmg（通用）', note_mac: 'Apple 芯片与 Intel 通用',
      dl_win: '下载安装程序', note_win: 'Windows 10 / 11，64 位',
      note_deb: 'Debian / Ubuntu', note_rpm: 'Fedora / RHEL', note_appimage: '其它发行版',
      f1t: '打开即看', f1d: '内存映射读取，多核并行解压，后台预解码相邻 Z 层，拖拉条直接出图。',
      f2t: 'Z / T 拉条', f2d: '拖动或用方向键翻层，空格播放；带时间维度的数据另有 T 拉条。',
      f3t: '多通道合成', f3d: '勾选显示通道、换颜色，每个通道单独调亮度、对比度和 γ。',
      f4t: '文件夹浏览', f4d: '侧边缩略图列出同目录的图像，上下键切换文件，不用回到 Finder。',
      f5t: '标尺与测量', f5d: '按像素尺寸显示比例尺，画线测长度，一键导出整图或选区为 PNG。',
      f6t: '格式齐全', f6d: 'TIFF（LZW/Deflate/JPEG，8–32 位）、OME-TIFF、ImageJ hyperstack、MRC/MRC2014，以及常见图片格式。',
      k_file: '上一个 / 下一个文件', k_z: '切换 Z 层', space: '空格', k_play: '播放 / 暂停 Z',
      or_dbl: '或双击', k_fit: '适应窗口', k_save: '保存 PNG 到当前目录', k_open: '打开文件',
      free: '免费开源'
    },
    en: {
      badge: 'Installers are just {range}',
      title: 'TifMrcView · Fast TIFF / MRC Viewer',
      desc: 'TifMrcView: a small, fast viewer for TIFF, OME-TIFF and MRC images with a Z slider, multi-channel compositing and folder browsing. Free for macOS, Windows and Linux.',
      nav_features: 'Features', nav_keys: 'Shortcuts', nav_download: 'Download',
      h1: 'TIFF and MRC,<br>as fast as they should be',
      lead: 'A small, fast microscopy image viewer. Double-click a .tif / .tiff / .mrc to open it, drag the slider through Z, composite channels in color, and step through every image in a folder.',
      cta: 'Free download', cta_os: 'Download for {os}', all: 'All versions', version: 'Version',
      mac_sub: 'macOS 11 or later', win_sub: 'No admin rights needed',
      mac_tip: 'If macOS blocks the first launch, go to System Settings → Privacy & Security and click “Open Anyway”, or run ' + XATTR + ' in Terminal.',
      win_tip: 'If another app already opens .tif files, click “Set as default” in TifMrcView and pick it in Windows Settings.',
      linux_tip: 'The AppImage bundles its libraries: run <code>chmod +x</code> on it and start it.',
      dl_mac: 'Download .dmg (Universal)', note_mac: 'Apple silicon and Intel',
      dl_win: 'Download installer', note_win: 'Windows 10 / 11, 64-bit',
      note_deb: 'Debian / Ubuntu', note_rpm: 'Fedora / RHEL', note_appimage: 'Other distributions',
      f1t: 'Opens instantly', f1d: 'Memory-mapped reads, parallel decompression on all cores, and neighbouring Z slices decoded ahead in the background.',
      f2t: 'Z / T sliders', f2d: 'Drag or use the arrow keys to move through slices, press Space to play; time series get a T slider too.',
      f3t: 'Multi-channel', f3d: 'Toggle channels, pick colors, and set brightness, contrast and gamma per channel.',
      f4t: 'Folder browsing', f4d: 'A thumbnail sidebar lists the images in the folder; switch files with the up/down keys.',
      f5t: 'Scale bar & measuring', f5d: 'Scale bar from pixel size, line measurements, and one-click PNG export of the whole image or a region.',
      f6t: 'All the formats', f6d: 'TIFF (LZW/Deflate/JPEG, 8–32 bit), OME-TIFF, ImageJ hyperstacks, MRC/MRC2014, plus common image formats.',
      k_file: 'Previous / next file', k_z: 'Change Z slice', space: 'Space', k_play: 'Play / pause Z',
      or_dbl: 'or double-click', k_fit: 'Fit to window', k_save: 'Save PNG to current folder', k_open: 'Open file',
      free: 'Free and open source'
    },
    de: {
      badge: 'Installer nur {range} groß',
      title: 'TifMrcView · Schneller TIFF- / MRC-Viewer',
      desc: 'TifMrcView: ein kleiner, schneller Viewer für TIFF, OME-TIFF und MRC mit Z-Regler, Mehrkanal-Überlagerung und Ordnernavigation. Kostenlos für macOS, Windows und Linux.',
      nav_features: 'Funktionen', nav_keys: 'Tastenkürzel', nav_download: 'Download',
      h1: 'TIFF und MRC –<br>endlich schnell',
      lead: 'Ein kleiner, schneller Viewer für Mikroskopiebilder. Doppelklick auf .tif / .tiff / .mrc öffnet die Datei, der Regler blättert durch Z, Kanäle werden farbig überlagert, und alle Bilder eines Ordners sind nur einen Tastendruck entfernt.',
      cta: 'Kostenlos herunterladen', cta_os: 'Für {os} herunterladen', all: 'Alle Versionen', version: 'Version',
      mac_sub: 'macOS 11 oder neuer', win_sub: 'Keine Administratorrechte nötig',
      mac_tip: 'Blockiert macOS den ersten Start, unter Systemeinstellungen → Datenschutz & Sicherheit auf „Dennoch öffnen“ klicken oder im Terminal ' + XATTR + ' ausführen.',
      win_tip: 'Wenn .tif bereits mit einem anderen Programm verknüpft ist, in TifMrcView auf „Als Standard festlegen“ klicken und in den Windows-Einstellungen auswählen.',
      linux_tip: 'Das AppImage bringt seine Bibliotheken mit: <code>chmod +x</code> ausführen und starten.',
      dl_mac: '.dmg herunterladen (Universal)', note_mac: 'Apple Silicon und Intel',
      dl_win: 'Installer herunterladen', note_win: 'Windows 10 / 11, 64-Bit',
      note_deb: 'Debian / Ubuntu', note_rpm: 'Fedora / RHEL', note_appimage: 'Andere Distributionen',
      f1t: 'Sofort offen', f1d: 'Memory-Mapping, parallele Dekompression auf allen Kernen und Vorausdekodierung benachbarter Z-Ebenen im Hintergrund.',
      f2t: 'Z- / T-Regler', f2d: 'Mit Regler oder Pfeiltasten durch die Ebenen, Leertaste zum Abspielen; Zeitreihen bekommen einen T-Regler.',
      f3t: 'Mehrkanal', f3d: 'Kanäle ein- und ausblenden, Farben wählen, Helligkeit, Kontrast und Gamma je Kanal einstellen.',
      f4t: 'Ordner durchblättern', f4d: 'Eine Miniaturleiste zeigt alle Bilder im Ordner; mit ↑/↓ zur nächsten Datei.',
      f5t: 'Maßstab & Messen', f5d: 'Maßstabsleiste aus der Pixelgröße, Längenmessung und PNG-Export des ganzen Bildes oder eines Bereichs.',
      f6t: 'Viele Formate', f6d: 'TIFF (LZW/Deflate/JPEG, 8–32 Bit), OME-TIFF, ImageJ-Hyperstacks, MRC/MRC2014 sowie gängige Bildformate.',
      k_file: 'Vorherige / nächste Datei', k_z: 'Z-Ebene wechseln', space: 'Leertaste', k_play: 'Z abspielen / pausieren',
      or_dbl: 'oder Doppelklick', k_fit: 'An Fenster anpassen', k_save: 'PNG im aktuellen Ordner speichern', k_open: 'Datei öffnen',
      free: 'Kostenlos und quelloffen'
    },
    ja: {
      badge: 'インストーラはわずか {range}',
      title: 'TifMrcView · 高速な TIFF / MRC ビューア',
      desc: 'TifMrcView：TIFF・OME-TIFF・MRC を軽快に表示するビューア。Z スライダー、マルチチャンネル合成、フォルダ内ブラウズに対応。macOS・Windows・Linux 版を無料でダウンロード。',
      nav_features: '機能', nav_keys: 'ショートカット', nav_download: 'ダウンロード',
      h1: 'TIFF と MRC を、<br>もっと速く',
      lead: '小さくて速い顕微鏡画像ビューア。.tif / .tiff / .mrc をダブルクリックで開き、スライダーで Z を送り、チャンネルをカラー合成。同じフォルダの画像もすぐ切り替えられます。',
      cta: '無料ダウンロード', cta_os: '{os} 版をダウンロード', all: 'すべてのバージョン', version: 'バージョン',
      mac_sub: 'macOS 11 以降', win_sub: '管理者権限は不要',
      mac_tip: '初回起動がブロックされた場合は「システム設定 → プライバシーとセキュリティ」で「このまま開く」を押すか、ターミナルで ' + XATTR + ' を実行してください。',
      win_tip: '.tif が他のアプリに関連付けられている場合は、TifMrcView の「既定に設定」を押して Windows の設定で選んでください。',
      linux_tip: 'AppImage は必要なライブラリを同梱しています。<code>chmod +x</code> してから実行してください。',
      dl_mac: '.dmg をダウンロード（Universal）', note_mac: 'Apple シリコン / Intel 両対応',
      dl_win: 'インストーラをダウンロード', note_win: 'Windows 10 / 11（64 ビット）',
      note_deb: 'Debian / Ubuntu', note_rpm: 'Fedora / RHEL', note_appimage: 'その他のディストリビューション',
      f1t: 'すぐに開く', f1d: 'メモリマップ読み込みと全コア並列展開、隣接する Z 層をバックグラウンドで先読み。',
      f2t: 'Z / T スライダー', f2d: 'スライダーか矢印キーで層を移動、スペースで再生。時系列データには T スライダーも。',
      f3t: 'マルチチャンネル', f3d: 'チャンネルの表示切り替えと色の変更、チャンネルごとに明るさ・コントラスト・γ を調整。',
      f4t: 'フォルダ内ブラウズ', f4d: 'サイドバーに同じフォルダの画像をサムネイル表示、↑↓ キーでファイルを切り替え。',
      f5t: 'スケールバーと計測', f5d: 'ピクセルサイズからスケールバーを表示、線で長さを計測、全体または選択範囲を PNG で書き出し。',
      f6t: '幅広い形式', f6d: 'TIFF（LZW/Deflate/JPEG、8–32 ビット）、OME-TIFF、ImageJ hyperstack、MRC/MRC2014、一般的な画像形式。',
      k_file: '前 / 次のファイル', k_z: 'Z 層を切り替え', space: 'スペース', k_play: 'Z の再生 / 一時停止',
      or_dbl: 'またはダブルクリック', k_fit: 'ウィンドウに合わせる', k_save: '現在のフォルダに PNG を保存', k_open: 'ファイルを開く',
      free: '無料・オープンソース'
    },
    es: {
      badge: 'Instaladores de solo {range}',
      title: 'TifMrcView · Visor rápido de TIFF / MRC',
      desc: 'TifMrcView: un visor pequeño y rápido para TIFF, OME-TIFF y MRC con control deslizante de Z, composición multicanal y navegación por carpetas. Gratis para macOS, Windows y Linux.',
      nav_features: 'Funciones', nav_keys: 'Atajos', nav_download: 'Descargar',
      h1: 'TIFF y MRC,<br>tan rápido como debe ser',
      lead: 'Un visor de imágenes de microscopía pequeño y rápido. Haz doble clic en un .tif / .tiff / .mrc para abrirlo, recorre Z con el deslizador, combina canales en color y pasa de una imagen a otra dentro de la carpeta.',
      cta: 'Descarga gratuita', cta_os: 'Descargar para {os}', all: 'Todas las versiones', version: 'Versión',
      mac_sub: 'macOS 11 o posterior', win_sub: 'No requiere permisos de administrador',
      mac_tip: 'Si macOS bloquea el primer inicio, ve a Ajustes del Sistema → Privacidad y seguridad y pulsa «Abrir igualmente», o ejecuta ' + XATTR + ' en Terminal.',
      win_tip: 'Si otra aplicación ya abre los .tif, pulsa «Establecer como predeterminada» en TifMrcView y elígela en Configuración de Windows.',
      linux_tip: 'El AppImage incluye sus bibliotecas: ejecuta <code>chmod +x</code> y ábrelo.',
      dl_mac: 'Descargar .dmg (Universal)', note_mac: 'Apple Silicon e Intel',
      dl_win: 'Descargar instalador', note_win: 'Windows 10 / 11, 64 bits',
      note_deb: 'Debian / Ubuntu', note_rpm: 'Fedora / RHEL', note_appimage: 'Otras distribuciones',
      f1t: 'Se abre al instante', f1d: 'Lectura mapeada en memoria, descompresión en paralelo en todos los núcleos y decodificación anticipada de los cortes Z vecinos.',
      f2t: 'Deslizadores Z / T', f2d: 'Arrastra o usa las flechas para cambiar de corte y Espacio para reproducir; las series temporales tienen deslizador T.',
      f3t: 'Multicanal', f3d: 'Activa canales, cambia colores y ajusta brillo, contraste y gamma por canal.',
      f4t: 'Navegar carpetas', f4d: 'Una barra lateral de miniaturas muestra las imágenes de la carpeta; cambia de archivo con ↑/↓.',
      f5t: 'Escala y medición', f5d: 'Barra de escala según el tamaño de píxel, medición de líneas y exportación a PNG de la imagen o de una región.',
      f6t: 'Todos los formatos', f6d: 'TIFF (LZW/Deflate/JPEG, 8–32 bits), OME-TIFF, hyperstacks de ImageJ, MRC/MRC2014 y formatos de imagen comunes.',
      k_file: 'Archivo anterior / siguiente', k_z: 'Cambiar corte Z', space: 'Espacio', k_play: 'Reproducir / pausar Z',
      or_dbl: 'o doble clic', k_fit: 'Ajustar a la ventana', k_save: 'Guardar PNG en la carpeta actual', k_open: 'Abrir archivo',
      free: 'Gratis y de código abierto'
    },
    fr: {
      badge: 'Installateurs de seulement {range}',
      title: 'TifMrcView · Visionneuse TIFF / MRC rapide',
      desc: 'TifMrcView : une visionneuse légère et rapide pour TIFF, OME-TIFF et MRC, avec curseur Z, composition multicanal et navigation dans les dossiers. Gratuite pour macOS, Windows et Linux.',
      nav_features: 'Fonctions', nav_keys: 'Raccourcis', nav_download: 'Télécharger',
      h1: 'TIFF et MRC,<br>enfin rapides',
      lead: 'Une visionneuse d’images de microscopie légère et rapide. Double-cliquez sur un .tif / .tiff / .mrc pour l’ouvrir, parcourez Z avec le curseur, composez les canaux en couleur et passez d’une image à l’autre dans le dossier.',
      cta: 'Téléchargement gratuit', cta_os: 'Télécharger pour {os}', all: 'Toutes les versions', version: 'Version',
      mac_sub: 'macOS 11 ou ultérieur', win_sub: 'Aucun droit administrateur requis',
      mac_tip: 'Si macOS bloque le premier lancement, allez dans Réglages Système → Confidentialité et sécurité et cliquez sur « Ouvrir quand même », ou exécutez ' + XATTR + ' dans le Terminal.',
      win_tip: 'Si une autre application ouvre déjà les .tif, cliquez sur « Définir par défaut » dans TifMrcView puis choisissez-la dans les Paramètres Windows.',
      linux_tip: 'L’AppImage embarque ses bibliothèques : faites <code>chmod +x</code> puis lancez-la.',
      dl_mac: 'Télécharger le .dmg (Universel)', note_mac: 'Puces Apple et Intel',
      dl_win: 'Télécharger l’installateur', note_win: 'Windows 10 / 11, 64 bits',
      note_deb: 'Debian / Ubuntu', note_rpm: 'Fedora / RHEL', note_appimage: 'Autres distributions',
      f1t: 'Ouverture immédiate', f1d: 'Lecture par mappage mémoire, décompression parallèle sur tous les cœurs et pré-décodage des plans Z voisins en arrière-plan.',
      f2t: 'Curseurs Z / T', f2d: 'Faites glisser ou utilisez les flèches pour changer de plan, Espace pour lire ; les séries temporelles ont un curseur T.',
      f3t: 'Multicanal', f3d: 'Affichez ou masquez les canaux, changez les couleurs, réglez luminosité, contraste et gamma par canal.',
      f4t: 'Parcourir le dossier', f4d: 'Une barre de vignettes liste les images du dossier ; changez de fichier avec ↑/↓.',
      f5t: 'Échelle et mesures', f5d: 'Barre d’échelle selon la taille de pixel, mesure de longueurs et export PNG de l’image entière ou d’une zone.',
      f6t: 'Tous les formats', f6d: 'TIFF (LZW/Deflate/JPEG, 8–32 bits), OME-TIFF, hyperstacks ImageJ, MRC/MRC2014 et formats d’image courants.',
      k_file: 'Fichier précédent / suivant', k_z: 'Changer de plan Z', space: 'Espace', k_play: 'Lire / mettre en pause Z',
      or_dbl: 'ou double-clic', k_fit: 'Ajuster à la fenêtre', k_save: 'Enregistrer en PNG dans le dossier courant', k_open: 'Ouvrir un fichier',
      free: 'Gratuit et open source'
    },
    ru: {
      badge: 'Установщик всего {range}',
      title: 'TifMrcView · Быстрый просмотрщик TIFF / MRC',
      desc: 'TifMrcView — компактный и быстрый просмотрщик TIFF, OME-TIFF и MRC с ползунком Z, многоканальным наложением и просмотром папки. Бесплатно для macOS, Windows и Linux.',
      nav_features: 'Возможности', nav_keys: 'Клавиши', nav_download: 'Скачать',
      h1: 'TIFF и MRC —<br>так быстро, как нужно',
      lead: 'Компактный и быстрый просмотрщик микроскопических изображений. Откройте .tif / .tiff / .mrc двойным щелчком, листайте Z ползунком, совмещайте каналы в цвете и переключайтесь между изображениями в папке.',
      cta: 'Скачать бесплатно', cta_os: 'Скачать для {os}', all: 'Все версии', version: 'Версия',
      mac_sub: 'macOS 11 и новее', win_sub: 'Права администратора не нужны',
      mac_tip: 'Если macOS блокирует первый запуск, откройте «Системные настройки → Конфиденциальность и безопасность» и нажмите «Всё равно открыть» или выполните в Терминале ' + XATTR,
      win_tip: 'Если .tif уже открывается другой программой, нажмите в TifMrcView «Сделать по умолчанию» и выберите её в параметрах Windows.',
      linux_tip: 'AppImage содержит все библиотеки: выполните <code>chmod +x</code> и запустите.',
      dl_mac: 'Скачать .dmg (Universal)', note_mac: 'Apple Silicon и Intel',
      dl_win: 'Скачать установщик', note_win: 'Windows 10 / 11, 64 бит',
      note_deb: 'Debian / Ubuntu', note_rpm: 'Fedora / RHEL', note_appimage: 'Другие дистрибутивы',
      f1t: 'Мгновенное открытие', f1d: 'Чтение через отображение в память, параллельная распаковка на всех ядрах и фоновое декодирование соседних слоёв Z.',
      f2t: 'Ползунки Z / T', f2d: 'Листайте слои ползунком или стрелками, пробел — воспроизведение; для временных рядов есть ползунок T.',
      f3t: 'Многоканальность', f3d: 'Включайте каналы, меняйте цвета, настраивайте яркость, контраст и гамму для каждого канала.',
      f4t: 'Просмотр папки', f4d: 'Боковая панель миниатюр показывает изображения папки; ↑/↓ — переключение файлов.',
      f5t: 'Масштаб и измерения', f5d: 'Масштабная линейка по размеру пикселя, измерение длин и экспорт всего изображения или области в PNG.',
      f6t: 'Все форматы', f6d: 'TIFF (LZW/Deflate/JPEG, 8–32 бит), OME-TIFF, гиперстеки ImageJ, MRC/MRC2014 и обычные форматы изображений.',
      k_file: 'Предыдущий / следующий файл', k_z: 'Сменить слой Z', space: 'Пробел', k_play: 'Воспроизвести / пауза Z',
      or_dbl: 'или двойной щелчок', k_fit: 'По размеру окна', k_save: 'Сохранить PNG в текущую папку', k_open: 'Открыть файл',
      free: 'Бесплатно и с открытым кодом'
    }
  };
  // Strings for the plugin market (site/plugins/) and the teaser on the home page.
  var EXTRA = {
    zh: {
      nav_home: '首页', nav_plugins: '插件',
      pl_t: '插件市场', pl_d: '录制视频、蔡司 SmartSEM 和 ZEN 采集参数……按需下载，在应用里一键加载。', pl_btn: '浏览插件',
      pl_title: '插件市场 · TifMrcView', pl_desc: '为 TifMrcView 下载扩展插件：录制视频、蔡司 SmartSEM / ZEN 采集参数等，并附带插件开发文档。',
      pl_h1: '插件市场', pl_lead: '插件给 TifMrcView 增加功能：把 Z / T 序列录成视频、读取蔡司电镜和共聚焦的采集参数……下载 zip，在应用里加载即可，不需要重装。',
      pl_install_h: '怎么安装',
      pl_s1: '在下面选一个插件，下载它的 <b>.zip</b> 文件（不要解压）。',
      pl_s2: '在 TifMrcView 里打开 <b>插件</b> → <b>加载插件…</b>，选择刚下载的 zip。',
      pl_s3: '插件马上生效；可以在插件列表里随时关闭或打开。',
      pl_update: '更新：再次加载新版本的 zip 即可覆盖旧版。',
      pl_remove: '删除：在插件列表里点 <b>卸载</b>。',
      pl_need: '需要 TifMrcView {v} 或更高版本', pl_need_all: '各插件需要的最低应用版本见卡片。',
      pl_builtin: '内置', pl_builtin_note: '已包含在应用里，无需下载',
      pl_native: '用到内置原生支持', pl_native_tip: '这个插件的原生部分已随应用发布，所以只能在官方版本里使用。',
      pl_download: '下载 .zip', pl_by: '作者 {a}', pl_more: '详细说明', pl_ver: '版本 {v}',
      pl_fail: '暂时无法加载插件列表，请稍后刷新重试。', pl_loading: '正在加载插件列表…', pl_empty: '还没有可下载的插件。',
      pl_dev_h: '制作插件 / 开发者文档', pl_dev_note: '',
      pl_list_h: '可用插件'
    },
    en: {
      nav_home: 'Home', nav_plugins: 'Plugins',
      pl_t: 'Plugin market', pl_d: 'Video recording, Zeiss SmartSEM and ZEN acquisition settings and more: download what you need and load it in the app.', pl_btn: 'Browse plugins',
      pl_title: 'Plugin market · TifMrcView', pl_desc: 'Download extension plugins for TifMrcView: video recording, Zeiss SmartSEM / ZEN acquisition settings and more, plus the plugin developer docs.',
      pl_h1: 'Plugin market', pl_lead: 'Plugins add features to TifMrcView: record Z / T series as video, read acquisition settings from Zeiss SEMs and confocals. Download a zip and load it in the app, no reinstall needed.',
      pl_install_h: 'How to install',
      pl_s1: 'Pick a plugin below and download its <b>.zip</b> (do not unzip it).',
      pl_s2: 'In TifMrcView open <b>Plugins</b> → <b>Install plugin…</b> and choose the zip.',
      pl_s3: 'It takes effect right away; switch it off or on any time in the plugin list.',
      pl_update: 'To update, load the newer zip; it replaces the old version.',
      pl_remove: 'To remove, click <b>Remove</b> in the plugin list.',
      pl_need: 'Needs TifMrcView {v} or later', pl_need_all: 'Each card shows the minimum app version it needs.',
      pl_builtin: 'Built-in', pl_builtin_note: 'Already part of the app, nothing to download',
      pl_native: 'Uses built-in native support', pl_native_tip: 'The native part of this plugin ships inside the app, so it only works in official builds.',
      pl_download: 'Download .zip', pl_by: 'by {a}', pl_more: 'Details', pl_ver: 'Version {v}',
      pl_fail: 'The plugin list could not be loaded right now. Please refresh and try again.', pl_loading: 'Loading plugins…', pl_empty: 'No plugins to download yet.',
      pl_dev_h: 'Make a plugin / developer docs', pl_dev_note: '',
      pl_list_h: 'Available plugins'
    },
    de: {
      nav_home: 'Start', nav_plugins: 'Plugins',
      pl_t: 'Plugin-Markt', pl_d: 'Videoaufnahme, Aufnahmeparameter aus Zeiss SmartSEM und ZEN und mehr: herunterladen und in der App laden.', pl_btn: 'Plugins ansehen',
      pl_title: 'Plugin-Markt · TifMrcView', pl_desc: 'Erweiterungen für TifMrcView herunterladen: Videoaufnahme, Aufnahmeparameter aus Zeiss SmartSEM / ZEN und mehr, dazu die Entwicklerdoku.',
      pl_h1: 'Plugin-Markt', pl_lead: 'Plugins erweitern TifMrcView: Z-/T-Serien als Video aufnehmen, Aufnahmeparameter von Zeiss-REM und -Konfokalmikroskopen lesen. ZIP herunterladen und in der App laden, ohne Neuinstallation.',
      pl_install_h: 'So installierst du es',
      pl_s1: 'Wähle unten ein Plugin und lade die <b>.zip</b> herunter (nicht entpacken).',
      pl_s2: 'Öffne in TifMrcView <b>Plugins</b> → <b>Plugin installieren…</b> und wähle die ZIP-Datei.',
      pl_s3: 'Es wirkt sofort; in der Plugin-Liste lässt es sich jederzeit aus- und einschalten.',
      pl_update: 'Zum Aktualisieren die neuere ZIP laden; sie ersetzt die alte Version.',
      pl_remove: 'Zum Entfernen in der Plugin-Liste auf <b>Entfernen</b> klicken.',
      pl_need: 'Benötigt TifMrcView {v} oder neuer', pl_need_all: 'Jede Karte nennt die minimal nötige App-Version.',
      pl_builtin: 'Integriert', pl_builtin_note: 'Schon in der App enthalten, kein Download nötig',
      pl_native: 'Nutzt integrierte native Unterstützung', pl_native_tip: 'Der native Teil dieses Plugins steckt in der App und funktioniert daher nur in offiziellen Builds.',
      pl_download: '.zip herunterladen', pl_by: 'von {a}', pl_more: 'Details', pl_ver: 'Version {v}',
      pl_fail: 'Die Plugin-Liste konnte gerade nicht geladen werden. Bitte neu laden und erneut versuchen.', pl_loading: 'Plugins werden geladen…', pl_empty: 'Noch keine Plugins zum Download.',
      pl_dev_h: 'Plugin entwickeln / Entwicklerdoku', pl_dev_note: 'Die Entwicklerdoku gibt es auf Englisch und Chinesisch.',
      pl_list_h: 'Verfügbare Plugins'
    },
    ja: {
      nav_home: 'ホーム', nav_plugins: 'プラグイン',
      pl_t: 'プラグインマーケット', pl_d: '動画録画、Zeiss SmartSEM・ZEN の取得パラメータなど。必要なものをダウンロードして、アプリで読み込みます。', pl_btn: 'プラグインを見る',
      pl_title: 'プラグインマーケット · TifMrcView', pl_desc: 'TifMrcView の拡張プラグインをダウンロード：動画録画、Zeiss SmartSEM / ZEN の取得パラメータなど。開発者向けドキュメントも掲載。',
      pl_h1: 'プラグインマーケット', pl_lead: 'プラグインで TifMrcView に機能を追加できます。Z / T シリーズの動画録画、Zeiss の SEM や共焦点顕微鏡の取得パラメータの読み取りなど。zip をダウンロードしてアプリで読み込むだけで、再インストールは不要です。',
      pl_install_h: 'インストール方法',
      pl_s1: '下からプラグインを選び、<b>.zip</b> をダウンロードします（解凍しないでください）。',
      pl_s2: 'TifMrcView で <b>プラグイン</b> → <b>プラグインをインストール…</b> を開き、zip を選びます。',
      pl_s3: 'すぐに有効になります。プラグイン一覧でいつでもオン／オフできます。',
      pl_update: '更新するには、新しい zip を読み込むと古いバージョンが置き換わります。',
      pl_remove: '削除するには、プラグイン一覧の <b>削除</b> を押します。',
      pl_need: 'TifMrcView {v} 以降が必要', pl_need_all: '各カードに必要な最低アプリバージョンを表示しています。',
      pl_builtin: '内蔵', pl_builtin_note: 'アプリに含まれています。ダウンロード不要',
      pl_native: '内蔵のネイティブ機能を使用', pl_native_tip: 'このプラグインのネイティブ部分はアプリに同梱されているため、公式ビルドでのみ動作します。',
      pl_download: '.zip をダウンロード', pl_by: '作者 {a}', pl_more: '詳細', pl_ver: 'バージョン {v}',
      pl_fail: 'プラグイン一覧を読み込めませんでした。再読み込みしてもう一度お試しください。', pl_loading: 'プラグインを読み込み中…', pl_empty: 'ダウンロードできるプラグインはまだありません。',
      pl_dev_h: 'プラグインを作る / 開発者向けドキュメント', pl_dev_note: '開発者向けドキュメントは英語と中国語です。',
      pl_list_h: '利用できるプラグイン'
    },
    es: {
      nav_home: 'Inicio', nav_plugins: 'Complementos',
      pl_t: 'Mercado de complementos', pl_d: 'Grabación de vídeo, parámetros de adquisición de Zeiss SmartSEM y ZEN y más: descarga lo que necesites y cárgalo en la app.', pl_btn: 'Ver complementos',
      pl_title: 'Mercado de complementos · TifMrcView', pl_desc: 'Descarga complementos para TifMrcView: grabación de vídeo, parámetros de adquisición de Zeiss SmartSEM / ZEN y más, con la documentación para desarrolladores.',
      pl_h1: 'Mercado de complementos', pl_lead: 'Los complementos añaden funciones a TifMrcView: grabar series Z / T en vídeo, leer los parámetros de adquisición de SEM y confocales Zeiss. Descarga un zip y cárgalo en la app, sin reinstalar.',
      pl_install_h: 'Cómo instalar',
      pl_s1: 'Elige un complemento abajo y descarga su <b>.zip</b> (no lo descomprimas).',
      pl_s2: 'En TifMrcView abre <b>Complementos</b> → <b>Instalar complemento…</b> y elige el zip.',
      pl_s3: 'Funciona de inmediato; puedes desactivarlo o activarlo cuando quieras en la lista.',
      pl_update: 'Para actualizar, carga el zip más nuevo: reemplaza la versión anterior.',
      pl_remove: 'Para quitarlo, pulsa <b>Quitar</b> en la lista de complementos.',
      pl_need: 'Requiere TifMrcView {v} o posterior', pl_need_all: 'Cada tarjeta indica la versión mínima de la app.',
      pl_builtin: 'Integrado', pl_builtin_note: 'Ya forma parte de la app, no hay nada que descargar',
      pl_native: 'Usa soporte nativo integrado', pl_native_tip: 'La parte nativa de este complemento viene dentro de la app, por lo que solo funciona en las versiones oficiales.',
      pl_download: 'Descargar .zip', pl_by: 'por {a}', pl_more: 'Detalles', pl_ver: 'Versión {v}',
      pl_fail: 'No se pudo cargar la lista de complementos. Actualiza la página e inténtalo de nuevo.', pl_loading: 'Cargando complementos…', pl_empty: 'Aún no hay complementos para descargar.',
      pl_dev_h: 'Crear un complemento / documentación para desarrolladores', pl_dev_note: 'La documentación para desarrolladores está en inglés y chino.',
      pl_list_h: 'Complementos disponibles'
    },
    fr: {
      nav_home: 'Accueil', nav_plugins: 'Extensions',
      pl_t: 'Marché des extensions', pl_d: 'Enregistrement vidéo, paramètres d’acquisition Zeiss SmartSEM et ZEN, et plus : téléchargez ce qu’il vous faut et chargez-le dans l’appli.', pl_btn: 'Voir les extensions',
      pl_title: 'Marché des extensions · TifMrcView', pl_desc: 'Téléchargez des extensions pour TifMrcView : enregistrement vidéo, paramètres d’acquisition Zeiss SmartSEM / ZEN, et plus, avec la documentation développeur.',
      pl_h1: 'Marché des extensions', pl_lead: 'Les extensions ajoutent des fonctions à TifMrcView : enregistrer des séries Z / T en vidéo, lire les paramètres d’acquisition des MEB et confocaux Zeiss. Téléchargez un zip et chargez-le dans l’appli, sans réinstaller.',
      pl_install_h: 'Comment installer',
      pl_s1: 'Choisissez une extension ci-dessous et téléchargez son <b>.zip</b> (ne le décompressez pas).',
      pl_s2: 'Dans TifMrcView, ouvrez <b>Extensions</b> → <b>Installer une extension…</b> et choisissez le zip.',
      pl_s3: 'Elle est active immédiatement ; vous pouvez la désactiver ou la réactiver à tout moment dans la liste.',
      pl_update: 'Pour mettre à jour, chargez le zip plus récent : il remplace l’ancienne version.',
      pl_remove: 'Pour la supprimer, cliquez sur <b>Supprimer</b> dans la liste des extensions.',
      pl_need: 'Nécessite TifMrcView {v} ou plus récent', pl_need_all: 'Chaque carte indique la version minimale de l’appli.',
      pl_builtin: 'Intégré', pl_builtin_note: 'Déjà inclus dans l’appli, rien à télécharger',
      pl_native: 'Utilise le support natif intégré', pl_native_tip: 'La partie native de cette extension est livrée dans l’appli ; elle ne fonctionne donc que dans les versions officielles.',
      pl_download: 'Télécharger le .zip', pl_by: 'par {a}', pl_more: 'Détails', pl_ver: 'Version {v}',
      pl_fail: 'Impossible de charger la liste des extensions pour le moment. Actualisez la page et réessayez.', pl_loading: 'Chargement des extensions…', pl_empty: 'Aucune extension à télécharger pour l’instant.',
      pl_dev_h: 'Créer une extension / documentation développeur', pl_dev_note: 'La documentation développeur est en anglais et en chinois.',
      pl_list_h: 'Extensions disponibles'
    },
    ru: {
      nav_home: 'Главная', nav_plugins: 'Плагины',
      pl_t: 'Каталог плагинов', pl_d: 'Запись видео, параметры съёмки Zeiss SmartSEM и ZEN и другое: скачайте нужное и загрузите в приложении.', pl_btn: 'Смотреть плагины',
      pl_title: 'Каталог плагинов · TifMrcView', pl_desc: 'Скачивайте плагины для TifMrcView: запись видео, параметры съёмки Zeiss SmartSEM / ZEN и другое, а также документацию для разработчиков.',
      pl_h1: 'Каталог плагинов', pl_lead: 'Плагины добавляют TifMrcView новые возможности: запись Z/T-серий в видео, чтение параметров съёмки СЭМ и конфокальных микроскопов Zeiss. Скачайте zip и загрузите в приложении, переустановка не нужна.',
      pl_install_h: 'Как установить',
      pl_s1: 'Выберите плагин ниже и скачайте его <b>.zip</b> (не распаковывайте).',
      pl_s2: 'В TifMrcView откройте <b>Плагины</b> → <b>Установить плагин…</b> и выберите zip.',
      pl_s3: 'Он начнёт работать сразу; в списке плагинов его можно в любой момент выключить и включить.',
      pl_update: 'Чтобы обновить, загрузите более новый zip: он заменит старую версию.',
      pl_remove: 'Чтобы удалить, нажмите <b>Удалить</b> в списке плагинов.',
      pl_need: 'Нужен TifMrcView {v} или новее', pl_need_all: 'На каждой карточке указана минимальная версия приложения.',
      pl_builtin: 'Встроен', pl_builtin_note: 'Уже входит в приложение, скачивать не нужно',
      pl_native: 'Использует встроенную нативную поддержку', pl_native_tip: 'Нативная часть этого плагина поставляется внутри приложения, поэтому он работает только в официальных сборках.',
      pl_download: 'Скачать .zip', pl_by: 'автор {a}', pl_more: 'Подробнее', pl_ver: 'Версия {v}',
      pl_fail: 'Сейчас не удалось загрузить список плагинов. Обновите страницу и повторите попытку.', pl_loading: 'Загрузка плагинов…', pl_empty: 'Пока нет плагинов для скачивания.',
      pl_dev_h: 'Создать плагин / документация разработчика', pl_dev_note: 'Документация для разработчиков доступна на английском и китайском.',
      pl_list_h: 'Доступные плагины'
    }
  };
  Object.keys(EXTRA).forEach(function (c) { for (var k in EXTRA[c]) T[c][k] = EXTRA[c][k]; });
  // Developer docs (site/plugins/docs.js) exist in Chinese and English only.
  if (window.TMV_DOCS) { T.zh.docs = window.TMV_DOCS.zh; T.en.docs = window.TMV_DOCS.en; }

  var LANGS = ['zh', 'en', 'de', 'ja', 'es', 'fr', 'ru'];
  var OS_NAMES = { mac: 'macOS', win: 'Windows', linux: 'Linux' };

  function pick() {
    try { var saved = localStorage.getItem('lang'); if (T[saved]) return saved; } catch (e) {}
    var q = (location.search.match(/[?&]lang=(\w+)/) || [])[1];
    if (T[q]) return q;
    var prefs = navigator.languages || [navigator.language || 'en'];
    for (var i = 0; i < prefs.length; i++) {
      var code = String(prefs[i]).toLowerCase().slice(0, 2);
      if (T[code]) return code;
    }
    return 'en';
  }

  var ua = navigator.userAgent;
  var os = /Mac/.test(ua) ? 'mac' : /Win/.test(ua) ? 'win' : /Linux|X11/.test(ua) ? 'linux' : null;
  var BASE = document.documentElement.getAttribute('data-base') || '';
  var HOME = !!document.getElementById('cta');
  var data = null, lang = pick(), sizes = {}, listeners = [];
  function mb(n) { return (n / 1048576).toFixed(n < 10485760 ? 1 : 0) + ' MB'; }
  function loadSizes(d) {
    var all = [];
    ['mac', 'win', 'linux'].forEach(function (k) { (d.files[k] || []).forEach(function (f) { all.push(f); }); });
    return Promise.all(all.map(function (f) {
      if (f.size) { sizes[f.file] = f.size; return null; }
      return fetch(f.file, { method: 'HEAD' }).then(function (r) {
        var n = +r.headers.get('content-length'); if (r.ok && n) sizes[f.file] = n;
      }).catch(function () {});
    }));
  }

  function flag(code) { return '<svg class="flag" viewBox="0 0 30 20">' + FLAGS[code] + '</svg>'; }

  function fileLabel(k, f, t) {
    var ext = (f.file.match(/\.(\w+)$/) || [])[1] || '';
    ext = ext.toLowerCase();
    if (k === 'mac') return [t.dl_mac, t.note_mac];
    if (k === 'win') return [t.dl_win, t.note_win];
    if (ext === 'deb') return ['.deb', t.note_deb];
    if (ext === 'rpm') return ['.rpm', t.note_rpm];
    if (ext === 'appimage') return ['.AppImage', t.note_appimage];
    return [f.label || f.file, f.note || ''];
  }

  function renderDownloads() {
    if (!data || !HOME) return;
    var t = T[lang];
    ['mac', 'win', 'linux'].forEach(function (k) {
      var box = document.querySelector('[data-os="' + k + '"] .dl');
      box.innerHTML = '';
      (data.files[k] || []).forEach(function (f) {
        var l = fileLabel(k, f, t);
        var a = document.createElement('a');
        a.href = f.file; a.setAttribute('download', '');
        var s = document.createElement('span'); s.textContent = l[0];
        var n = document.createElement('small'); n.textContent = l[1];
        if (sizes[f.file]) { var b = document.createElement('b'); b.textContent = mb(sizes[f.file]); n.appendChild(b); }
        a.appendChild(s); a.appendChild(n); box.appendChild(a);
      });
    });
    var small = Object.keys(sizes).filter(function (k) { return !/appimage$/i.test(k); }).map(function (k) { return sizes[k]; });
    var badge = document.getElementById('badge');
    if (small.length) {
      var lo = Math.min.apply(null, small), hi = Math.max.apply(null, small);
      var range = mb(lo) === mb(hi) ? mb(hi) : mb(lo).replace(' MB', '') + '–' + mb(hi);
      badge.textContent = '⚡ ' + t.badge.replace('{range}', range);
      badge.hidden = false;
    }
    document.getElementById('meta').textContent = t.version + ' ' + data.version + ' · ' + data.date + ' · macOS · Windows · Linux';
    document.getElementById('ver').textContent = 'v' + data.version;
    var c = document.getElementById('cta');
    if (os && data.files[os] && data.files[os][0]) {
      c.href = data.files[os][0].file; c.setAttribute('download', '');
      c.textContent = t.cta_os.replace('{os}', OS_NAMES[os]);
      document.querySelector('[data-os="' + os + '"]').classList.add('hl');
    }
  }

  function apply(code) {
    lang = code;
    var t = T[code];
    document.documentElement.lang = HTML_LANG[code];
    var page = document.documentElement.getAttribute('data-page');
    document.title = page === 'plugins' ? t.pl_title : t.title;
    var md = document.querySelector('meta[name="description"]'); if (md) md.content = page === 'plugins' ? t.pl_desc : t.desc;
    document.querySelectorAll('[data-i]').forEach(function (el) {
      var key = el.getAttribute('data-i');
      var v = t[key];
      if (v == null && code !== 'zh') v = T.en[key];
      if (v != null) el.innerHTML = v;
    });
    var btn = document.querySelector('#lang > button');
    btn.querySelector('.flag').outerHTML = flag(code);
    btn.querySelector('span').textContent = NAMES[code];
    document.querySelectorAll('#lang li button').forEach(function (b) {
      b.setAttribute('aria-current', b.dataset.lang === code ? 'true' : 'false');
    });
    renderDownloads();
    listeners.forEach(function (fn) { fn(code); });
  }

  // language menu
  var box = document.getElementById('lang');
  var ul = box.querySelector('ul');
  ul.innerHTML = LANGS.map(function (c) {
    return '<li><button type="button" role="menuitem" data-lang="' + c + '" lang="' + HTML_LANG[c] + '">' + flag(c) + NAMES[c] + '</button></li>';
  }).join('');
  var toggle = box.querySelector(':scope > button');
  toggle.addEventListener('click', function (e) {
    e.stopPropagation();
    var open = box.classList.toggle('open');
    toggle.setAttribute('aria-expanded', open);
  });
  ul.addEventListener('click', function (e) {
    var b = e.target.closest('button[data-lang]'); if (!b) return;
    try { localStorage.setItem('lang', b.dataset.lang); } catch (err) {}
    apply(b.dataset.lang);
    box.classList.remove('open'); toggle.setAttribute('aria-expanded', 'false');
  });
  document.addEventListener('click', function () { box.classList.remove('open'); toggle.setAttribute('aria-expanded', 'false'); });
  document.addEventListener('keydown', function (e) { if (e.key === 'Escape') box.classList.remove('open'); });

  apply(lang);
  window.TMV = {
    lang: function () { return lang; },
    t: function (key, vars) {
      var v = T[lang][key]; if (v == null) v = T.en[key]; if (v == null) v = '';
      for (var k in (vars || {})) v = v.replace('{' + k + '}', vars[k]);
      return v;
    },
    onLang: function (fn) { listeners.push(fn); fn(lang); }
  };
  fetch(BASE + 'downloads.json', { cache: 'no-cache' }).then(function (r) { return r.json(); })
    .then(function (d) {
      data = d;
      var v = document.getElementById('ver'); if (v) v.textContent = 'v' + d.version;
      renderDownloads();
      return HOME ? loadSizes(d) : null;
    })
    .then(renderDownloads).catch(function () {});
})();
