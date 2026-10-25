"use strict";

// Built-in plugin: translates the interface. The app is written in Chinese;
// this plugin watches the DOM and swaps every known Chinese string (text,
// `title` and `placeholder`) for the chosen language, so it also covers
// other plugins' UI and messages that arrive later (toasts, status lines).
//
// To add a string, add a row to WORDS (exact text) or PATTERNS (text with
// numbers/names in it; `$1`… are the captures, which are translated too).
// Columns follow LANGS.

const LANGS = [
  ["zh", "中文"],
  ["en", "English"],
  ["de", "Deutsch"],
  ["ja", "日本語"],
  ["es", "Español"],
  ["fr", "Français"],
  ["ru", "Русский"],
];

// prettier-ignore
const WORDS = [
  // toolbar
  ["打开", "Open", "Öffnen", "開く", "Abrir", "Ouvrir", "Открыть"],
  ["打开文件 (Ctrl/⌘+O)", "Open file (Ctrl/⌘+O)", "Datei öffnen (Strg/⌘+O)", "ファイルを開く (Ctrl/⌘+O)", "Abrir archivo (Ctrl/⌘+O)", "Ouvrir un fichier (Ctrl/⌘+O)", "Открыть файл (Ctrl/⌘+O)"],
  ["上一个文件 (↑)", "Previous file (↑)", "Vorherige Datei (↑)", "前のファイル (↑)", "Archivo anterior (↑)", "Fichier précédent (↑)", "Предыдущий файл (↑)"],
  ["下一个文件 (↓)", "Next file (↓)", "Nächste Datei (↓)", "次のファイル (↓)", "Archivo siguiente (↓)", "Fichier suivant (↓)", "Следующий файл (↓)"],
  ["工具", "Tools", "Werkzeuge", "ツール", "Herramientas", "Outils", "Инструменты"],
  ["工具：亮度对比度、测量、比例尺、存图 (T)", "Tools: brightness/contrast, measure, scale bar, export (T)", "Werkzeuge: Helligkeit/Kontrast, Messen, Maßstab, Export (T)", "ツール：明るさ/コントラスト、計測、スケールバー、書き出し (T)", "Herramientas: brillo/contraste, medir, barra de escala, exportar (T)", "Outils : luminosité/contraste, mesure, barre d'échelle, export (T)", "Инструменты: яркость/контраст, измерение, масштаб, экспорт (T)"],
  ["适应", "Fit", "Einpassen", "フィット", "Ajustar", "Ajuster", "Вписать"],
  ["适应窗口 (F)", "Fit to window (F)", "An Fenster anpassen (F)", "ウィンドウに合わせる (F)", "Ajustar a la ventana (F)", "Ajuster à la fenêtre (F)", "По размеру окна (F)"],
  ["原始大小 (1)", "Actual size (1)", "Originalgröße (1)", "原寸 (1)", "Tamaño real (1)", "Taille réelle (1)", "Реальный размер (1)"],
  ["预览栏", "Thumbnails", "Vorschau", "サムネイル", "Miniaturas", "Vignettes", "Миниатюры"],
  ["显示/收起右侧预览栏 (B)", "Show/hide the thumbnail sidebar (B)", "Vorschauleiste ein-/ausblenden (B)", "サムネイルバーの表示/非表示 (B)", "Mostrar/ocultar la barra de miniaturas (B)", "Afficher/masquer la barre de vignettes (B)", "Показать/скрыть панель миниатюр (B)"],
  ["设为默认", "Set as default", "Als Standard", "既定に設定", "Predeterminar", "Par défaut", "По умолчанию"],
  ["把 TifMrcView 设为 .tif/.tiff/.mrc 的默认打开方式", "Make TifMrcView the default app for .tif/.tiff/.mrc", "TifMrcView als Standard-App für .tif/.tiff/.mrc festlegen", "TifMrcView を .tif/.tiff/.mrc の既定のアプリにする", "Usar TifMrcView como aplicación predeterminada para .tif/.tiff/.mrc", "Faire de TifMrcView l'application par défaut pour .tif/.tiff/.mrc", "Сделать TifMrcView приложением по умолчанию для .tif/.tiff/.mrc"],
  ["插件", "Plugins", "Plugins", "プラグイン", "Complementos", "Extensions", "Плагины"],
  ["插件与更新", "Plugins and updates", "Plugins und Updates", "プラグインとアップデート", "Complementos y actualizaciones", "Extensions et mises à jour", "Плагины и обновления"],
  ["界面语言", "Interface language", "Sprache der Oberfläche", "表示言語", "Idioma de la interfaz", "Langue de l'interface", "Язык интерфейса"],
  // update bar
  ["立即更新并重启", "Update and restart", "Aktualisieren und neu starten", "今すぐ更新して再起動", "Actualizar y reiniciar", "Mettre à jour et redémarrer", "Обновить и перезапустить"],
  ["忽略这个版本", "Skip this version", "Diese Version überspringen", "このバージョンをスキップ", "Omitir esta versión", "Ignorer cette version", "Пропустить эту версию"],
  // empty stage
  ["拖入或双击打开图像", "Drop an image here or double-click to open", "Bild hierher ziehen oder doppelklicken", "画像をドロップするかダブルクリックで開く", "Arrastra una imagen o haz doble clic para abrir", "Déposez une image ou double-cliquez pour ouvrir", "Перетащите изображение или дважды щёлкните"],
  ["支持 tif / tiff / mrc / mrcs / png / jpg / bmp / gif / webp", "Supports tif / tiff / mrc / mrcs / png / jpg / bmp / gif / webp", "Unterstützt tif / tiff / mrc / mrcs / png / jpg / bmp / gif / webp", "対応形式 tif / tiff / mrc / mrcs / png / jpg / bmp / gif / webp", "Admite tif / tiff / mrc / mrcs / png / jpg / bmp / gif / webp", "Formats : tif / tiff / mrc / mrcs / png / jpg / bmp / gif / webp", "Форматы: tif / tiff / mrc / mrcs / png / jpg / bmp / gif / webp"],
  ["↑ ↓ 切换文件　← → 切换 Z　空格 播放　滚轮 缩放　拖动 平移　Ctrl/⌘+S 存 PNG",
    "↑ ↓ file　← → Z　Space play　Wheel zoom　Drag pan　Ctrl/⌘+S save PNG",
    "↑ ↓ Datei　← → Z　Leertaste abspielen　Mausrad zoomen　Ziehen verschieben　Strg/⌘+S PNG speichern",
    "↑ ↓ ファイル切替　← → Z 切替　スペース 再生　ホイール ズーム　ドラッグ 移動　Ctrl/⌘+S PNG 保存",
    "↑ ↓ archivo　← → Z　Espacio reproducir　Rueda zoom　Arrastrar desplazar　Ctrl/⌘+S guardar PNG",
    "↑ ↓ fichier　← → Z　Espace lecture　Molette zoom　Glisser déplacer　Ctrl/⌘+S PNG",
    "↑ ↓ файл　← → Z　Пробел воспроизвести　Колесо масштаб　Перетаскивание сдвиг　Ctrl/⌘+S сохранить PNG"],
  ["加载中…", "Loading…", "Wird geladen…", "読み込み中…", "Cargando…", "Chargement…", "Загрузка…"],
  ["保存中…", "Saving…", "Wird gespeichert…", "保存中…", "Guardando…", "Enregistrement…", "Сохранение…"],
  ["无法预览", "No preview", "Keine Vorschau", "プレビュー不可", "Sin vista previa", "Pas d'aperçu", "Нет превью"],
  ["灰度", "Gray", "Grau", "グレー", "Gris", "Gris", "Серый"],
  ["勾选：显示/隐藏　点名字：调这个通道的亮度对比度　点圆点：换颜色", "Checkbox: show/hide　Name: adjust this channel's contrast　Dot: change color", "Häkchen: ein/aus　Name: Kontrast dieses Kanals　Punkt: Farbe ändern", "チェック：表示/非表示　名前：このチャンネルのコントラスト　丸：色を変更", "Casilla: mostrar/ocultar　Nombre: contraste de este canal　Punto: cambiar color", "Case : afficher/masquer　Nom : contraste de ce canal　Point : changer la couleur", "Флажок: показать/скрыть　Имя: контраст канала　Точка: сменить цвет"],
  // brightness / contrast
  ["亮度 / 对比度", "Brightness / Contrast", "Helligkeit / Kontrast", "明るさ / コントラスト", "Brillo / Contraste", "Luminosité / Contraste", "Яркость / Контраст"],
  ["在直方图上拖动可直接选择显示范围", "Drag on the histogram to pick the display range", "Im Histogramm ziehen, um den Anzeigebereich zu wählen", "ヒストグラム上をドラッグして表示範囲を選択", "Arrastra en el histograma para elegir el rango", "Glissez sur l'histogramme pour choisir la plage", "Потяните по гистограмме, чтобы выбрать диапазон"],
  ["自动（每层）", "Auto (per slice)", "Auto (je Ebene)", "自動（スライスごと）", "Auto (por corte)", "Auto (par coupe)", "Авто (для каждого слоя)"],
  ["重置", "Reset", "Zurücksetzen", "リセット", "Restablecer", "Réinitialiser", "Сбросить"],
  ["显示最小值", "Display minimum", "Anzeigeminimum", "表示最小値", "Mínimo mostrado", "Minimum affiché", "Минимум отображения"],
  ["显示最大值", "Display maximum", "Anzeigemaximum", "表示最大値", "Máximo mostrado", "Maximum affiché", "Максимум отображения"],
  ["亮度", "Brightness", "Helligkeit", "明るさ", "Brillo", "Luminosité", "Яркость"],
  ["对比度", "Contrast", "Kontrast", "コントラスト", "Contraste", "Contraste", "Контраст"],
  // measure & annotate
  ["测量与标注", "Measure & annotate", "Messen & Beschriften", "計測と注釈", "Medir y anotar", "Mesure et annotation", "Измерение и пометки"],
  ["移动", "Pan", "Verschieben", "移動", "Mover", "Déplacer", "Сдвиг"],
  ["移动 (V)", "Pan (V)", "Verschieben (V)", "移動 (V)", "Mover (V)", "Déplacer (V)", "Сдвиг (V)"],
  ["测距", "Measure", "Messen", "計測", "Medir", "Mesurer", "Линейка"],
  ["测距 (M)", "Measure distance (M)", "Abstand messen (M)", "距離を計測 (M)", "Medir distancia (M)", "Mesurer une distance (M)", "Измерить расстояние (M)"],
  ["画笔", "Pen", "Stift", "ペン", "Lápiz", "Crayon", "Перо"],
  ["画笔 (P)", "Pen (P)", "Stift (P)", "ペン (P)", "Lápiz (P)", "Crayon (P)", "Перо (P)"],
  ["选区", "Select", "Auswahl", "選択", "Selección", "Sélection", "Выделение"],
  ["框选区域，用于存局部图 (R)", "Select a region to export (R)", "Bereich für den Export auswählen (R)", "書き出す範囲を選択 (R)", "Selecciona una región para exportar (R)", "Sélectionner une zone à exporter (R)", "Выделить область для экспорта (R)"],
  ["颜色", "Color", "Farbe", "色", "Color", "Couleur", "Цвет"],
  ["线宽", "Width", "Breite", "線幅", "Grosor", "Épaisseur", "Толщина"],
  ["显示标注 (H)", "Show annotations (H)", "Beschriftungen zeigen (H)", "注釈を表示 (H)", "Mostrar anotaciones (H)", "Afficher les annotations (H)", "Показать пометки (H)"],
  ["撤销", "Undo", "Rückgängig", "元に戻す", "Deshacer", "Annuler", "Отменить"],
  ["撤销 (Ctrl/⌘+Z)", "Undo (Ctrl/⌘+Z)", "Rückgängig (Strg/⌘+Z)", "元に戻す (Ctrl/⌘+Z)", "Deshacer (Ctrl/⌘+Z)", "Annuler (Ctrl/⌘+Z)", "Отменить (Ctrl/⌘+Z)"],
  ["清除", "Clear", "Löschen", "消去", "Borrar", "Effacer", "Очистить"],
  // IMOD
  ["IMOD 模型", "IMOD model", "IMOD-Modell", "IMOD モデル", "Modelo IMOD", "Modèle IMOD", "Модель IMOD"],
  ["显示模型 (I)", "Show model (I)", "Modell zeigen (I)", "モデルを表示 (I)", "Mostrar modelo (I)", "Afficher le modèle (I)", "Показать модель (I)"],
  ["加载模型…", "Load model…", "Modell laden…", "モデルを読み込む…", "Cargar modelo…", "Charger un modèle…", "Загрузить модель…"],
  ["同名的 .mod / .fid 会自动加载", "A .mod / .fid with the same name loads automatically", "Gleichnamige .mod / .fid werden automatisch geladen", "同名の .mod / .fid は自動で読み込まれます", "Un .mod / .fid con el mismo nombre se carga solo", "Un .mod / .fid du même nom est chargé automatiquement", "Одноимённый .mod / .fid загружается автоматически"],
  // scale bar
  ["比例尺", "Scale bar", "Maßstab", "スケールバー", "Barra de escala", "Barre d'échelle", "Масштабная линейка"],
  ["显示比例尺", "Show scale bar", "Maßstab zeigen", "スケールバーを表示", "Mostrar barra de escala", "Afficher la barre d'échelle", "Показать линейку"],
  ["像素尺寸", "Pixel size", "Pixelgröße", "ピクセルサイズ", "Tamaño de píxel", "Taille de pixel", "Размер пикселя"],
  ["未知", "unknown", "unbekannt", "不明", "desconocido", "inconnue", "неизвестно"],
  ["像素尺寸来自文件", "Pixel size from the file", "Pixelgröße aus der Datei", "ピクセルサイズはファイルから", "Tamaño de píxel del archivo", "Taille de pixel issue du fichier", "Размер пикселя из файла"],
  ["像素尺寸手动", "Pixel size set by hand", "Pixelgröße manuell", "ピクセルサイズは手動", "Tamaño de píxel manual", "Taille de pixel manuelle", "Размер пикселя задан вручную"],
  ["文件里没有像素尺寸，可以手动填写", "The file has no pixel size; you can enter it", "Die Datei enthält keine Pixelgröße; bitte eingeben", "ファイルにピクセルサイズがありません。手動で入力できます", "El archivo no tiene tamaño de píxel; puedes escribirlo", "Le fichier n'a pas de taille de pixel ; saisissez-la", "В файле нет размера пикселя, введите вручную"],
  // export
  ["存图", "Export", "Export", "書き出し", "Exportar", "Exporter", "Экспорт"],
  ["存整张…", "Save whole image…", "Ganzes Bild speichern…", "全体を保存…", "Guardar imagen completa…", "Enregistrer l'image…", "Сохранить целиком…"],
  ["存选区…", "Save selection…", "Auswahl speichern…", "選択範囲を保存…", "Guardar selección…", "Enregistrer la sélection…", "Сохранить выделение…"],
  ["先用「选区」框出区域", "Draw a region with Select first", "Zuerst mit Auswahl einen Bereich ziehen", "先に「選択」で範囲を囲んでください", "Primero dibuja una región con Selección", "Tracez d'abord une zone avec Sélection", "Сначала выделите область"],
  ["按原始分辨率导出 PNG，显示中的标注和比例尺会一起保存。Ctrl/⌘+S 直接存到图片所在文件夹。",
    "Exports PNG at full resolution, with the visible annotations and scale bar. Ctrl/⌘+S saves next to the image.",
    "Exportiert PNG in voller Auflösung mit sichtbaren Beschriftungen und Maßstab. Strg/⌘+S speichert neben dem Bild.",
    "元の解像度で PNG を書き出します。表示中の注釈とスケールバーも含まれます。Ctrl/⌘+S で画像と同じフォルダに保存。",
    "Exporta PNG a resolución completa, con las anotaciones y la barra de escala visibles. Ctrl/⌘+S guarda junto a la imagen.",
    "Exporte un PNG en pleine résolution, avec les annotations et la barre d'échelle visibles. Ctrl/⌘+S enregistre à côté de l'image.",
    "Экспорт PNG в исходном разрешении вместе с видимыми пометками и линейкой. Ctrl/⌘+S сохраняет рядом с изображением."],
  ["播放 Z (空格)", "Play Z (Space)", "Z abspielen (Leertaste)", "Z を再生 (スペース)", "Reproducir Z (Espacio)", "Lire Z (Espace)", "Воспроизвести Z (пробел)"],
  // plugin dialog & updates
  ["关闭 (Esc)", "Close (Esc)", "Schließen (Esc)", "閉じる (Esc)", "Cerrar (Esc)", "Fermer (Échap)", "Закрыть (Esc)"],
  ["打开插件文件夹", "Open plugin folder", "Plugin-Ordner öffnen", "プラグインフォルダを開く", "Abrir carpeta de complementos", "Ouvrir le dossier des extensions", "Открыть папку плагинов"],
  ["把 .js 插件放进这个文件夹，然后重新加载", "Put .js plugins in this folder, then reload", ".js-Plugins in diesen Ordner legen und neu laden", ".js プラグインをこのフォルダに入れてから再読み込み", "Pon los complementos .js en esta carpeta y recarga", "Placez les extensions .js dans ce dossier, puis rechargez", "Положите .js-плагины в эту папку и перезагрузите"],
  ["重新加载", "Reload", "Neu laden", "再読み込み", "Recargar", "Recharger", "Перезагрузить"],
  ["重新加载界面以载入新插件", "Reload the interface to pick up new plugins", "Oberfläche neu laden, um neue Plugins zu laden", "画面を再読み込みして新しいプラグインを読み込む", "Recarga la interfaz para cargar complementos nuevos", "Recharger l'interface pour charger les nouvelles extensions", "Перезагрузить интерфейс для новых плагинов"],
  ["没有插件", "No plugins", "Keine Plugins", "プラグインなし", "Sin complementos", "Aucune extension", "Нет плагинов"],
  ["内置", "built-in", "integriert", "内蔵", "integrado", "intégré", "встроенный"],
  ["更新", "Updates", "Updates", "アップデート", "Actualizaciones", "Mises à jour", "Обновления"],
  ["检查更新", "Check for updates", "Nach Updates suchen", "アップデートを確認", "Buscar actualizaciones", "Rechercher des mises à jour", "Проверить обновления"],
  ["启动时自动检查", "Check at startup", "Beim Start prüfen", "起動時に確認", "Comprobar al iniciar", "Vérifier au démarrage", "Проверять при запуске"],
  ["已是最新版本", "Up to date", "Aktuell", "最新です", "Actualizado", "À jour", "Последняя версия"],
  ["正在检查…", "Checking…", "Wird geprüft…", "確認中…", "Comprobando…", "Vérification…", "Проверка…"],
  ["正在下载更新…", "Downloading update…", "Update wird geladen…", "アップデートをダウンロード中…", "Descargando actualización…", "Téléchargement de la mise à jour…", "Загрузка обновления…"],
  // plugin names
  ["多语言", "Languages", "Sprachen", "多言語", "Idiomas", "Langues", "Языки"],
  ["界面语言：中文、English、Deutsch、日本語、Español、Français、Русский", "Interface language: 中文, English, Deutsch, 日本語, Español, Français, Русский", "Oberflächensprache: 中文, English, Deutsch, 日本語, Español, Français, Русский", "表示言語：中文、English、Deutsch、日本語、Español、Français、Русский", "Idioma: 中文, English, Deutsch, 日本語, Español, Français, Русский", "Langue : 中文, English, Deutsch, 日本語, Español, Français, Русский", "Язык: 中文, English, Deutsch, 日本語, Español, Français, Русский"],
  ["录制视频", "Video recorder", "Videoaufnahme", "動画録画", "Grabar vídeo", "Enregistrer une vidéo", "Запись видео"],
  ["把 Z / T 序列录成 MP4，带当前的对比度、通道、标注和比例尺", "Records the Z / T series as MP4 with the current contrast, channels, annotations and scale bar", "Nimmt die Z-/T-Serie als MP4 mit aktuellem Kontrast, Kanälen, Beschriftungen und Maßstab auf", "Z / T シリーズを現在のコントラスト・チャンネル・注釈・スケールバー付きで MP4 に録画", "Graba la serie Z / T en MP4 con el contraste, canales, anotaciones y barra de escala actuales", "Enregistre la série Z / T en MP4 avec le contraste, les canaux, les annotations et la barre d'échelle", "Записывает серию Z / T в MP4 с текущим контрастом, каналами, пометками и линейкой"],
  // recorder plugin
  ["录制视频 (MP4)", "Record video (MP4)", "Video aufnehmen (MP4)", "動画を録画 (MP4)", "Grabar vídeo (MP4)", "Enregistrer une vidéo (MP4)", "Запись видео (MP4)"],
  ["这个版本没有编译录制功能", "This build has no recorder", "Dieser Build enthält keine Aufnahme", "このビルドには録画機能がありません", "Esta versión no incluye grabación", "Cette version n'inclut pas l'enregistrement", "В этой сборке нет записи"],
  ["按哪个维度播放", "Axis to play", "Abzuspielende Achse", "再生する軸", "Eje a reproducir", "Axe à lire", "Ось воспроизведения"],
  ["Z 层", "Z slices", "Z-Ebenen", "Z スライス", "Cortes Z", "Coupes Z", "Слои Z"],
  ["时间 T", "Time T", "Zeit T", "時間 T", "Tiempo T", "Temps T", "Время T"],
  ["从第几帧", "First frame", "Erstes Bild", "開始フレーム", "Primer fotograma", "Première image", "Первый кадр"],
  ["到第几帧", "Last frame", "Letztes Bild", "終了フレーム", "Último fotograma", "Dernière image", "Последний кадр"],
  ["帧率", "Frame rate", "Bildrate", "フレームレート", "Fotogramas/s", "Images/s", "Частота кадров"],
  ["播完后倒着再播一遍", "Play backwards after the end", "Am Ende rückwärts abspielen", "最後まで再生したら逆再生", "Al terminar, reproducir hacia atrás", "Rejouer à l'envers à la fin", "Затем проиграть назад"],
  ["往返", "Bounce", "Hin und zurück", "往復", "Ida y vuelta", "Aller-retour", "Туда-обратно"],
  ["录哪一块", "Region to record", "Aufzunehmender Bereich", "録画する範囲", "Región a grabar", "Zone à enregistrer", "Область записи"],
  ["整张图", "Whole image", "Ganzes Bild", "画像全体", "Imagen completa", "Image entière", "Всё изображение"],
  ["当前视野", "Current view", "Aktuelle Ansicht", "現在の表示", "Vista actual", "Vue actuelle", "Текущий вид"],
  ["视频最长边", "Longest side of the video", "Längste Videoseite", "動画の長辺", "Lado mayor del vídeo", "Plus grand côté de la vidéo", "Длинная сторона видео"],
  ["原始尺寸", "Original size", "Originalgröße", "元のサイズ", "Tamaño original", "Taille d'origine", "Исходный размер"],
  ["画质", "Quality", "Qualität", "画質", "Calidad", "Qualité", "Качество"],
  ["高画质", "High", "Hoch", "高画質", "Alta", "Haute", "Высокое"],
  ["标准", "Standard", "Standard", "標準", "Estándar", "Standard", "Стандартное"],
  ["小文件", "Small file", "Kleine Datei", "小さいファイル", "Archivo pequeño", "Petit fichier", "Маленький файл"],
  ["录制 MP4…", "Record MP4…", "MP4 aufnehmen…", "MP4 を録画…", "Grabar MP4…", "Enregistrer en MP4…", "Записать MP4…"],
  ["停止", "Stop", "Stopp", "停止", "Detener", "Arrêter", "Стоп"],
  ["已停止", "Stopped", "Gestoppt", "停止しました", "Detenido", "Arrêté", "Остановлено"],
  ["已停止，未保存", "Stopped, nothing saved", "Gestoppt, nichts gespeichert", "停止しました（未保存）", "Detenido, no se guardó", "Arrêté, rien n'a été enregistré", "Остановлено, не сохранено"],
  ["这张图只有一层，没有可录的序列", "This image has a single slice; nothing to record", "Dieses Bild hat nur eine Ebene; nichts aufzunehmen", "この画像は 1 枚だけなので録画できません", "Esta imagen tiene un solo corte; nada que grabar", "Cette image n'a qu'une coupe ; rien à enregistrer", "В изображении один слой, записывать нечего"],
  ["至少要两帧", "Needs at least two frames", "Mindestens zwei Bilder nötig", "2 フレーム以上必要です", "Se necesitan al menos dos fotogramas", "Il faut au moins deux images", "Нужно минимум два кадра"],
  // quick toolbar, crop and saved annotations (PR #5)
  ["选择", "Select", "Auswählen", "選択", "Seleccionar", "Sélection", "Выбор"],
  ["画框", "Rectangle", "Rechteck", "四角形", "Rectángulo", "Rectangle", "Прямоугольник"],
  ["画圆", "Ellipse", "Ellipse", "楕円", "Elipse", "Ellipse", "Эллипс"],
  ["裁剪", "Crop", "Zuschneiden", "切り抜き", "Recortar", "Rogner", "Обрезка"],
  ["亮度对比度", "Contrast", "Kontrast", "コントラスト", "Contraste", "Contraste", "Контраст"],
  ["更多", "More", "Mehr", "その他", "Más", "Plus", "Ещё"],
  ["标注", "Annotations", "Beschriftungen", "注釈", "Anotaciones", "Annotations", "Пометки"],
  ["清除本层", "Clear slice", "Ebene leeren", "このスライスを消去", "Borrar corte", "Effacer la coupe", "Очистить слой"],
  ["选择：点选标注后拖动、缩放，拖端点重新测量，Delete 删除 (S)", "Select: click an annotation to move or resize it, drag an end to re-measure, Delete removes it (S)", "Auswählen: Beschriftung anklicken zum Verschieben/Skalieren, Endpunkt ziehen zum Neumessen, Entf löscht (S)", "選択：注釈をクリックして移動・拡大縮小、端点をドラッグで再計測、Delete で削除 (S)", "Seleccionar: haz clic en una anotación para moverla o escalarla, arrastra un extremo para volver a medir, Supr la borra (S)", "Sélection : cliquez une annotation pour la déplacer ou la redimensionner, faites glisser une extrémité pour remesurer, Suppr l'efface (S)", "Выбор: щёлкните пометку, чтобы двигать или менять размер, тяните конец для новой меры, Delete удаляет (S)"],
  ["画框 (R)，按住 Shift 画正方形", "Rectangle (R), hold Shift for a square", "Rechteck (R), Umschalt für Quadrat", "四角形 (R)、Shift で正方形", "Rectángulo (R), Mayús para un cuadrado", "Rectangle (R), Maj pour un carré", "Прямоугольник (R), Shift — квадрат"],
  ["画圆 (E)，按住 Shift 画正圆", "Ellipse (E), hold Shift for a circle", "Ellipse (E), Umschalt für Kreis", "楕円 (E)、Shift で正円", "Elipse (E), Mayús para un círculo", "Ellipse (E), Maj pour un cercle", "Эллипс (E), Shift — круг"],
  ["裁剪 (C)：框出区域后双击或回车裁剪，Esc / 退格撤回", "Crop (C): draw a region, then double-click or press Enter; Esc / Backspace undoes", "Zuschneiden (C): Bereich ziehen, dann Doppelklick oder Eingabe; Esc / Rücktaste macht rückgängig", "切り抜き (C)：範囲を囲んでダブルクリックか Enter、Esc / Backspace で元に戻す", "Recortar (C): dibuja una región y haz doble clic o pulsa Intro; Esc / Retroceso deshace", "Rogner (C) : tracez une zone puis double-cliquez ou appuyez sur Entrée ; Échap / Retour annule", "Обрезка (C): выделите область, затем двойной щелчок или Enter; Esc / Backspace отменяет"],
  ["标注颜色", "Annotation color", "Beschriftungsfarbe", "注釈の色", "Color de anotación", "Couleur des annotations", "Цвет пометок"],
  ["显示/隐藏比例尺（像素尺寸在「更多」里改）", "Show/hide scale bar (set the pixel size under More)", "Maßstab ein-/ausblenden (Pixelgröße unter Mehr)", "スケールバーの表示/非表示（ピクセルサイズは「その他」で変更）", "Mostrar/ocultar barra de escala (tamaño de píxel en Más)", "Afficher/masquer la barre d'échelle (taille de pixel dans Plus)", "Показать/скрыть линейку (размер пикселя в «Ещё»)"],
  ["存图：按原始分辨率存 PNG，裁剪后只存裁剪区域，带上显示中的标注和比例尺。Ctrl/⌘+S 直接存到图片所在文件夹",
    "Export: saves a full-resolution PNG (only the crop if cropped) with the visible annotations and scale bar. Ctrl/⌘+S saves next to the image",
    "Export: speichert ein PNG in voller Auflösung (nur Zuschnitt, falls zugeschnitten) mit sichtbaren Beschriftungen und Maßstab. Strg/⌘+S speichert neben dem Bild",
    "書き出し：元の解像度で PNG を保存（切り抜き時はその範囲のみ）。表示中の注釈とスケールバーも含みます。Ctrl/⌘+S で画像と同じフォルダに保存",
    "Exportar: guarda un PNG a resolución completa (solo el recorte si lo hay) con las anotaciones y la barra de escala visibles. Ctrl/⌘+S guarda junto a la imagen",
    "Exporter : enregistre un PNG en pleine résolution (seulement la zone rognée le cas échéant) avec les annotations et la barre d'échelle visibles. Ctrl/⌘+S enregistre à côté de l'image",
    "Экспорт: сохраняет PNG в исходном разрешении (только обрезку, если она есть) с видимыми пометками и линейкой. Ctrl/⌘+S сохраняет рядом с изображением"],
  ["显示/隐藏标注 (H)", "Show/hide annotations (H)", "Beschriftungen ein-/ausblenden (H)", "注釈の表示/非表示 (H)", "Mostrar/ocultar anotaciones (H)", "Afficher/masquer les annotations (H)", "Показать/скрыть пометки (H)"],
  ["更多：亮度对比度、标注、IMOD 模型、比例尺设置 (T)", "More: contrast, annotations, IMOD model, scale bar settings (T)", "Mehr: Kontrast, Beschriftungen, IMOD-Modell, Maßstab (T)", "その他：コントラスト、注釈、IMOD モデル、スケールバー設定 (T)", "Más: contraste, anotaciones, modelo IMOD, barra de escala (T)", "Plus : contraste, annotations, modèle IMOD, barre d'échelle (T)", "Ещё: контраст, пометки, модель IMOD, линейка (T)"],
  ["标注会在画完几秒后自动存到图片旁边的 .annot.json 文件", "Annotations are saved automatically to a .annot.json file next to the image a few seconds after drawing", "Beschriftungen werden kurz nach dem Zeichnen automatisch in einer .annot.json neben dem Bild gespeichert", "注釈は描いた数秒後に画像の隣の .annot.json に自動保存されます", "Las anotaciones se guardan solas en un .annot.json junto a la imagen unos segundos después de dibujar", "Les annotations sont enregistrées automatiquement dans un .annot.json à côté de l'image quelques secondes après le dessin", "Пометки автоматически сохраняются в .annot.json рядом с изображением через несколько секунд"],
  ["文件里没有像素尺寸，先在这里填写", "The file has no pixel size; enter it here first", "Die Datei enthält keine Pixelgröße; zuerst hier eingeben", "ファイルにピクセルサイズがありません。先にここで入力してください", "El archivo no tiene tamaño de píxel; escríbelo aquí primero", "Le fichier n'a pas de taille de pixel ; saisissez-la d'abord ici", "В файле нет размера пикселя, сначала введите его здесь"],
  // messages from the native side
  ["已将 TifMrcView 设为 .tif / .tiff / .mrc 的默认打开方式", "TifMrcView is now the default app for .tif / .tiff / .mrc", "TifMrcView ist jetzt Standard-App für .tif / .tiff / .mrc", "TifMrcView を .tif / .tiff / .mrc の既定のアプリにしました", "TifMrcView es ahora la aplicación predeterminada para .tif / .tiff / .mrc", "TifMrcView est maintenant l'application par défaut pour .tif / .tiff / .mrc", "TifMrcView теперь открывает .tif / .tiff / .mrc по умолчанию"],
  ["已打开系统“默认应用”设置，请在其中把 .tif / .tiff / .mrc 选为 TifMrcView", "Opened the system Default apps settings; choose TifMrcView for .tif / .tiff / .mrc there", "Systemeinstellung „Standard-Apps“ geöffnet; dort TifMrcView für .tif / .tiff / .mrc wählen", "システムの「既定のアプリ」設定を開きました。.tif / .tiff / .mrc に TifMrcView を選んでください", "Se abrió la configuración de Aplicaciones predeterminadas; elige TifMrcView para .tif / .tiff / .mrc", "Paramètres « Applications par défaut » ouverts ; choisissez TifMrcView pour .tif / .tiff / .mrc", "Открыты настройки «Приложения по умолчанию»; выберите там TifMrcView для .tif / .tiff / .mrc"],
  ["当前系统不支持", "Not supported on this system", "Auf diesem System nicht unterstützt", "このシステムでは非対応です", "No compatible con este sistema", "Non pris en charge sur ce système", "Не поддерживается в этой системе"],
  ["没有可安装的更新", "No update to install", "Kein Update zu installieren", "インストールできるアップデートはありません", "No hay actualización para instalar", "Aucune mise à jour à installer", "Нет обновлений для установки"],
  ["不是 IMOD 模型文件", "Not an IMOD model file", "Keine IMOD-Modelldatei", "IMOD モデルファイルではありません", "No es un archivo de modelo IMOD", "Ce n'est pas un modèle IMOD", "Это не файл модели IMOD"],
  ["模型文件被截断", "The model file is truncated", "Die Modelldatei ist abgeschnitten", "モデルファイルが途中で切れています", "El archivo de modelo está truncado", "Le fichier de modèle est tronqué", "Файл модели обрезан"],
  ["MRC 文件头不完整", "Incomplete MRC header", "Unvollständiger MRC-Header", "MRC ヘッダーが不完全です", "Cabecera MRC incompleta", "En-tête MRC incomplet", "Неполный заголовок MRC"],
  ["MRC 数据被截断", "The MRC data is truncated", "MRC-Daten abgeschnitten", "MRC データが途中で切れています", "Datos MRC truncados", "Données MRC tronquées", "Данные MRC обрезаны"],
];

// prettier-ignore
const PATTERNS = [
  // longer / more specific first
  [/^已保存 (.+)（(\d+)×(\d+)，(\d+) 帧，(\S+) MB，用时 (\S+) 秒）$/,
    "Saved $1 ($2×$3, $4 frames, $5 MB, $6 s)", "$1 gespeichert ($2×$3, $4 Bilder, $5 MB, $6 s)", "$1 を保存しました（$2×$3、$4 フレーム、$5 MB、$6 秒）",
    "Guardado $1 ($2×$3, $4 fotogramas, $5 MB, $6 s)", "$1 enregistré ($2×$3, $4 images, $5 Mo, $6 s)", "Сохранено $1 ($2×$3, $4 кадров, $5 МБ, $6 с)"],
  [/^无法打开 (.+?)：(.+)$/, "Cannot open $1: $2", "$1 kann nicht geöffnet werden: $2", "$1 を開けません：$2", "No se puede abrir $1: $2", "Impossible d'ouvrir $1 : $2", "Не удаётся открыть $1: $2"],
  [/^无法打开 (.+?)(: .*)?$/, "Cannot open $1$2", "$1 kann nicht geöffnet werden$2", "$1 を開けません$2", "No se puede abrir $1$2", "Impossible d'ouvrir $1$2", "Не удаётся открыть $1$2"],
  [/^无法读取 (.+?)(: .*)?$/, "Cannot read $1$2", "$1 kann nicht gelesen werden$2", "$1 を読めません$2", "No se puede leer $1$2", "Impossible de lire $1$2", "Не удаётся прочитать $1$2"],
  [/^无法读取模型：(.+)$/, "Cannot read the model: $1", "Modell kann nicht gelesen werden: $1", "モデルを読めません：$1", "No se puede leer el modelo: $1", "Impossible de lire le modèle : $1", "Не удаётся прочитать модель: $1"],
  [/^无法打开插件文件夹：(.+)$/, "Cannot open the plugin folder: $1", "Plugin-Ordner kann nicht geöffnet werden: $1", "プラグインフォルダを開けません：$1", "No se puede abrir la carpeta de complementos: $1", "Impossible d'ouvrir le dossier des extensions : $1", "Не удаётся открыть папку плагинов: $1"],
  [/^不支持的文件类型: (.+)$/, "Unsupported file type: $1", "Nicht unterstützter Dateityp: $1", "非対応のファイル形式：$1", "Tipo de archivo no compatible: $1", "Type de fichier non pris en charge : $1", "Неподдерживаемый тип файла: $1"],
  [/^部分类型设置失败: (.+)$/, "Some types could not be set: $1", "Einige Typen konnten nicht gesetzt werden: $1", "一部の形式を設定できませんでした：$1", "No se pudieron configurar algunos tipos: $1", "Certains types n'ont pas pu être définis : $1", "Не удалось назначить часть типов: $1"],
  [/^已加载 IMOD 模型 (.+)$/, "Loaded IMOD model $1", "IMOD-Modell $1 geladen", "IMOD モデル $1 を読み込みました", "Modelo IMOD $1 cargado", "Modèle IMOD $1 chargé", "Загружена модель IMOD $1"],
  [/^视频已保存 (.+)$/, "Video saved: $1", "Video gespeichert: $1", "動画を保存しました：$1", "Vídeo guardado: $1", "Vidéo enregistrée : $1", "Видео сохранено: $1"],
  [/^已保存 (.+)$/, "Saved $1", "$1 gespeichert", "$1 を保存しました", "Guardado $1", "$1 enregistré", "Сохранено: $1"],
  [/^已自动保存到 (.+)$/, "Saved automatically to $1", "Automatisch gespeichert in $1", "$1 に自動保存しました", "Guardado automáticamente en $1", "Enregistré automatiquement dans $1", "Автоматически сохранено в $1"],
  [/^从 (.+) 读入 (\d+) 个标注$/, "Loaded $2 annotations from $1", "$2 Beschriftungen aus $1 geladen", "$1 から注釈を $2 個読み込みました", "$2 anotaciones cargadas de $1", "$2 annotations chargées depuis $1", "Загружено пометок из $1: $2"],
  [/^已裁剪到 (\d+) × (\d+) px，按 Esc 或退格撤回$/, "Cropped to $1 × $2 px; Esc or Backspace undoes", "Auf $1 × $2 px zugeschnitten; Esc oder Rücktaste macht rückgängig", "$1 × $2 px に切り抜きました。Esc か Backspace で元に戻す", "Recortado a $1 × $2 px; Esc o Retroceso deshace", "Rogné à $1 × $2 px ; Échap ou Retour annule", "Обрезано до $1 × $2 px; Esc или Backspace отменяет"],
  [/^标注保存失败：(.+)$/, "Saving annotations failed: $1", "Speichern der Beschriftungen fehlgeschlagen: $1", "注釈の保存に失敗しました：$1", "Error al guardar las anotaciones: $1", "Échec de l'enregistrement des annotations : $1", "Не удалось сохранить пометки: $1"],
  [/^(.+) 读取失败$/, "Could not read $1", "$1 konnte nicht gelesen werden", "$1 を読めませんでした", "No se pudo leer $1", "Impossible de lire $1", "Не удалось прочитать $1"],
  [/^裁剪 (.+)$/, "Crop $1", "Zuschnitt $1", "切り抜き $1", "Recorte $1", "Rognage $1", "Обрезка $1"],
  [/^渲染失败：(.+)$/, "Render failed: $1", "Darstellung fehlgeschlagen: $1", "描画に失敗しました：$1", "Error al dibujar: $1", "Échec du rendu : $1", "Ошибка отрисовки: $1"],
  [/^保存失败：(.+)$/, "Save failed: $1", "Speichern fehlgeschlagen: $1", "保存に失敗しました：$1", "Error al guardar: $1", "Échec de l'enregistrement : $1", "Ошибка сохранения: $1"],
  [/^设置失败：(.+)$/, "Setting failed: $1", "Einstellen fehlgeschlagen: $1", "設定に失敗しました：$1", "Error de configuración: $1", "Échec du réglage : $1", "Ошибка настройки: $1"],
  [/^录制失败：(.+)$/, "Recording failed: $1", "Aufnahme fehlgeschlagen: $1", "録画に失敗しました：$1", "Error al grabar: $1", "Échec de l'enregistrement : $1", "Ошибка записи: $1"],
  [/^录制中 (\d+) \/ (\d+)$/, "Recording $1 / $2", "Aufnahme $1 / $2", "録画中 $1 / $2", "Grabando $1 / $2", "Enregistrement $1 / $2", "Запись $1 / $2"],
  [/^出错：(.+)$/, "Error: $1", "Fehler: $1", "エラー：$1", "Error: $1", "Erreur : $1", "Ошибка: $1"],
  [/^显示最小值（数据最小 (.+)）$/, "Display minimum (data min $1)", "Anzeigeminimum (Daten-Min. $1)", "表示最小値（データ最小 $1）", "Mínimo mostrado (mín. de datos $1)", "Minimum affiché (min. des données $1)", "Минимум отображения (минимум данных $1)"],
  [/^显示最大值（数据最大 (.+)）$/, "Display maximum (data max $1)", "Anzeigemaximum (Daten-Max. $1)", "表示最大値（データ最大 $1）", "Máximo mostrado (máx. de datos $1)", "Maximum affiché (max. des données $1)", "Максимум отображения (максимум данных $1)"],
  [/^(.+) · (\d+) 个对象 · (\d+) 个点(.*)$/, "$1 · $2 objects · $3 points$4", "$1 · $2 Objekte · $3 Punkte$4", "$1 · オブジェクト $2 個 · 点 $3 個$4", "$1 · $2 objetos · $3 puntos$4", "$1 · $2 objets · $3 points$4", "$1 · объектов: $2 · точек: $3$4"],
  [/^（模型建在 (\d+)×(\d+) 的图上，和当前图像大小不同）$/, " (model was made on a $1×$2 image, different from this one)", " (Modell für ein $1×$2-Bild, andere Größe als dieses)", "（モデルは $1×$2 の画像用で、現在の画像とサイズが異なります）", " (modelo creado sobre una imagen de $1×$2, distinta de esta)", " (modèle créé sur une image $1×$2, différente de celle-ci)", " (модель создана для изображения $1×$2, размер отличается)"],
  [/^对象 (\d+)$/, "Object $1", "Objekt $1", "オブジェクト $1", "Objeto $1", "Objet $1", "Объект $1"],
  [/^(.+)（(\d+)）$/, "$1 ($2)", "$1 ($2)", "$1（$2）", "$1 ($2)", "$1 ($2)", "$1 ($2)"],
  [/^选区 (\d+) × (\d+) px$/, "Selection $1 × $2 px", "Auswahl $1 × $2 px", "選択範囲 $1 × $2 px", "Selección $1 × $2 px", "Sélection $1 × $2 px", "Выделение $1 × $2 px"],
  [/^(x \S+  y \S+)  值 (.+)$/, "$1  value $2", "$1  Wert $2", "$1  値 $2", "$1  valor $2", "$1  valeur $2", "$1  значение $2"],
  [/^发现新版本 (\S+)（当前 (\S+)）$/, "Version $1 is available (you have $2)", "Version $1 verfügbar (installiert: $2)", "新しいバージョン $1 があります（現在 $2）", "Versión $1 disponible (tienes $2)", "Version $1 disponible (actuelle : $2)", "Доступна версия $1 (текущая $2)"],
  [/^可更新到 (\S+)（已忽略）$/, "$1 available (skipped)", "$1 verfügbar (übersprungen)", "$1 に更新可能（スキップ済み）", "$1 disponible (omitida)", "$1 disponible (ignorée)", "Доступна $1 (пропущена)"],
  [/^可更新到 (\S+)$/, "$1 available", "$1 verfügbar", "$1 に更新できます", "$1 disponible", "$1 disponible", "Доступна $1"],
  [/^检查更新失败：(.+)$/, "Update check failed: $1", "Update-Prüfung fehlgeschlagen: $1", "アップデートの確認に失敗しました：$1", "Error al buscar actualizaciones: $1", "Échec de la recherche de mises à jour : $1", "Не удалось проверить обновления: $1"],
  [/^检查失败：(.+)$/, "Check failed: $1", "Prüfung fehlgeschlagen: $1", "確認に失敗しました：$1", "Error al comprobar: $1", "Échec de la vérification : $1", "Ошибка проверки: $1"],
  [/^正在下载更新… (.+)$/, "Downloading update… $1", "Update wird geladen… $1", "アップデートをダウンロード中… $1", "Descargando actualización… $1", "Téléchargement de la mise à jour… $1", "Загрузка обновления… $1"],
  [/^更新失败：(.+)$/, "Update failed: $1", "Update fehlgeschlagen: $1", "更新に失敗しました：$1", "Error al actualizar: $1", "Échec de la mise à jour : $1", "Ошибка обновления: $1"],
  [/^(.+)  v(\S+)$/, "$1  v$2", "$1  v$2", "$1  v$2", "$1  v$2", "$1  v$2", "$1  v$2"],
  [/^(.+) · 内置$/, "$1 · built-in", "$1 · integriert", "$1 · 内蔵", "$1 · integrado", "$1 · intégré", "$1 · встроенный"],
];

const CJK = /[　-〿㐀-鿿＀-￯]/;
const ATTRS = ["title", "placeholder"];
const SKIP = "[data-no-i18n], #file-name, script, style";

/** Picks a supported language from the system's preferred languages. */
function systemLang() {
  for (const l of navigator.languages || [navigator.language || ""]) {
    const code = l.toLowerCase().split("-")[0];
    if (LANGS.some(([c]) => c === code)) return code;
  }
  return "en";
}

TifMrcView.register({
  id: "i18n",
  name: "多语言",
  version: "1.0.0",
  description: "界面语言：中文、English、Deutsch、日本語、Español、Français、Русский",

  activate(app) {
    let lang = app.settings.get("lang", null) || systemLang();
    let words = new Map();
    let patterns = [];
    const texts = new WeakMap(); // Text -> {orig, cur}
    const attrs = new WeakMap(); // Element -> {title: {orig, cur}, ...}

    function load(code) {
      lang = code;
      const col = LANGS.findIndex(([c]) => c === code);
      words = new Map(col > 0 ? WORDS.map((r) => [r[0], r[col]]) : []);
      patterns = col > 0 ? PATTERNS.map((r) => [r[0], r[col]]) : [];
      document.documentElement.lang = code === "zh" ? "zh-CN" : code;
    }

    function core(s) {
      if (!CJK.test(s)) return s;
      const w = words.get(s);
      if (w !== undefined) return w;
      // Icon prefix, e.g. "✋ 移动".
      const p = s.match(/^([^\p{L}\p{N}（]+)(.+)$/u);
      if (p && words.has(p[2])) return p[1] + words.get(p[2]);
      for (const [re, out] of patterns) {
        const m = s.match(re);
        if (m) return out.replace(/\$(\d)/g, (_, i) => core(m[+i] ?? ""));
      }
      return s;
    }

    function tr(s) {
      if (lang === "zh") return s;
      const m = s.match(/^(\s*)([\s\S]*?)(\s*)$/);
      return m[1] + core(m[2]) + m[3];
    }

    const skip = (el) => !el || !!el.closest(SKIP);

    function doText(n) {
      let st = texts.get(n);
      if (!st || n.data !== st.cur) {
        if (!CJK.test(n.data) || skip(n.parentElement)) return;
        st = { orig: n.data };
        texts.set(n, st);
      }
      st.cur = tr(st.orig);
      if (n.data !== st.cur) n.data = st.cur;
    }

    function doAttr(el, name) {
      const v = el.getAttribute(name);
      if (v === null) return;
      let m = attrs.get(el);
      let st = m && m[name];
      if (!st || v !== st.cur) {
        if (!CJK.test(v) || skip(el)) return;
        if (!m) attrs.set(el, (m = {}));
        st = m[name] = { orig: v };
      }
      st.cur = tr(st.orig);
      if (v !== st.cur) el.setAttribute(name, st.cur);
    }

    function walk(root) {
      if (root.nodeType === Node.TEXT_NODE) return doText(root);
      if (root.nodeType !== Node.ELEMENT_NODE) return;
      for (const a of ATTRS) doAttr(root, a);
      const it = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT);
      for (let n = it.nextNode(); n; n = it.nextNode()) {
        if (n.nodeType === Node.TEXT_NODE) doText(n);
        else for (const a of ATTRS) doAttr(n, a);
      }
    }

    const obs = new MutationObserver((records) => {
      for (const r of records) {
        if (r.type === "characterData") doText(r.target);
        else if (r.type === "attributes") doAttr(r.target, r.attributeName);
        else r.addedNodes.forEach(walk);
      }
      obs.takeRecords(); // drop the records our own writes just made
    });

    function setLang(code) {
      load(code);
      walk(document.body);
      obs.takeRecords();
    }

    app.ui.addToolbarSelect({
      title: "界面语言",
      options: LANGS,
      value: lang,
      onChange(code) {
        app.settings.set("lang", code);
        setLang(code);
      },
    });

    setLang(lang);
    obs.observe(document.body, {
      subtree: true,
      childList: true,
      characterData: true,
      attributes: true,
      attributeFilter: ATTRS,
    });
    this._off = () => {
      obs.disconnect();
      setLang("zh");
    };
  },

  deactivate() {
    if (this._off) this._off();
    this._off = null;
  },
});
