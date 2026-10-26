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
  ["这个文件里有多个图像，选择要看的那个", "This file holds several images; choose the one to view", "Diese Datei enthält mehrere Bilder; wählen Sie eines aus", "このファイルには複数の画像があります。表示する画像を選んでください", "Este archivo contiene varias imágenes; elija la que quiere ver", "Ce fichier contient plusieurs images ; choisissez celle à afficher", "В этом файле несколько изображений; выберите нужное"],
  ["图像", "Image", "Bild", "画像", "Imagen", "Image", "Изображение"],
  ["参考图像", "Reference image", "Referenzbild", "参照画像", "Imagen de referencia", "Image de référence", "Опорное изображение"],
  ["这个文件只有一个图像", "This file holds only one image", "Diese Datei enthält nur ein Bild", "このファイルには画像が 1 つしかありません", "Este archivo solo contiene una imagen", "Ce fichier ne contient qu'une image", "В этом файле только одно изображение"],
  ["工具", "Tools", "Werkzeuge", "ツール", "Herramientas", "Outils", "Инструменты"],
  ["工具：亮度对比度、测量、比例尺、存图 (T)", "Tools: brightness/contrast, measure, scale bar, export (T)", "Werkzeuge: Helligkeit/Kontrast, Messen, Maßstab, Export (T)", "ツール：明るさ/コントラスト、計測、スケールバー、書き出し (T)", "Herramientas: brillo/contraste, medir, barra de escala, exportar (T)", "Outils : luminosité/contraste, mesure, barre d'échelle, export (T)", "Инструменты: яркость/контраст, измерение, масштаб, экспорт (T)"],
  ["缩小 (-)", "Zoom out (-)", "Verkleinern (-)", "縮小 (-)", "Reducir (-)", "Zoom arrière (-)", "Уменьшить (-)"],
  ["放大 (+)", "Zoom in (+)", "Vergrößern (+)", "拡大 (+)", "Ampliar (+)", "Zoom avant (+)", "Увеличить (+)"],
  ["点击适应窗口 (F)；按 1 为 100%", "Click to fit window (F); press 1 for 100%", "Klicken: an Fenster anpassen (F); Taste 1 = 100 %", "クリックでウィンドウに合わせる (F)、1 キーで 100%", "Clic: ajustar a la ventana (F); tecla 1 = 100 %", "Cliquer : ajuster à la fenêtre (F) ; touche 1 = 100 %", "Клик: по размеру окна (F); клавиша 1 = 100%"],
  ["重做 (Ctrl/⌘+Shift+Z)", "Redo (Ctrl/⌘+Shift+Z)", "Wiederholen (Strg/⌘+Umschalt+Z)", "やり直し (Ctrl/⌘+Shift+Z)", "Rehacer (Ctrl/⌘+Mayús+Z)", "Rétablir (Ctrl/⌘+Maj+Z)", "Повторить (Ctrl/⌘+Shift+Z)"],
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
  ["支持 tif / tiff / mrc / mrcs / png / jpg / bmp / gif / webp；装上插件还能打开 czi 等格式", "Supports tif / tiff / mrc / mrcs / png / jpg / bmp / gif / webp; plugins add czi and more", "Unterstützt tif / tiff / mrc / mrcs / png / jpg / bmp / gif / webp; Plugins ergänzen czi und mehr", "対応形式 tif / tiff / mrc / mrcs / png / jpg / bmp / gif / webp。プラグインで czi なども開けます", "Admite tif / tiff / mrc / mrcs / png / jpg / bmp / gif / webp; los complementos añaden czi y más", "Formats : tif / tiff / mrc / mrcs / png / jpg / bmp / gif / webp ; les extensions ajoutent czi et d'autres", "Форматы: tif / tiff / mrc / mrcs / png / jpg / bmp / gif / webp; плагины добавляют czi и другие"],
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
    "Exports PNG at full resolution, with the visible annotations and scale bar. Ctrl/⌘+S saves to the save folder (the image's folder by default; change it in Tools).",
    "Exportiert PNG in voller Auflösung mit sichtbaren Beschriftungen und Maßstab. Strg/⌘+S speichert im Speicherordner (standardmäßig der Ordner des Bildes; änderbar unter Werkzeuge).",
    "元の解像度で PNG を書き出します。表示中の注釈とスケールバーも含まれます。Ctrl/⌘+S で保存フォルダに保存（既定は画像と同じフォルダ。「ツール」で変更可）。",
    "Exporta PNG a resolución completa, con las anotaciones y la barra de escala visibles. Ctrl/⌘+S guarda en la carpeta de guardado (por defecto, la de la imagen; se cambia en Herramientas).",
    "Exporte un PNG en pleine résolution, avec les annotations et la barre d'échelle visibles. Ctrl/⌘+S enregistre dans le dossier d'enregistrement (par défaut celui de l'image ; modifiable dans Outils).",
    "Экспорт PNG в исходном разрешении вместе с видимыми пометками и линейкой. Ctrl/⌘+S сохраняет в папку сохранения (по умолчанию папка изображения; меняется в «Инструментах»)."],
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
  ["无法获取更新信息：连不上更新服务器，或服务器上还没有发布版本，请稍后再试",
    "Could not get update information: the update server is unreachable or has no release published yet. Please try again later",
    "Update-Informationen nicht verfügbar: Update-Server nicht erreichbar oder noch keine Version veröffentlicht. Bitte später erneut versuchen",
    "アップデート情報を取得できません。アップデートサーバーに接続できないか、まだリリースが公開されていません。しばらくしてから再試行してください",
    "No se pudo obtener información de actualización: el servidor no responde o aún no hay ninguna versión publicada. Inténtalo más tarde",
    "Impossible d'obtenir les informations de mise à jour : serveur injoignable ou aucune version publiée pour l'instant. Réessayez plus tard",
    "Не удалось получить сведения об обновлении: сервер обновлений недоступен или версия ещё не опубликована. Повторите позже"],
  // plugin packages
  ["插件市场", "Plugin market", "Plugin-Markt", "プラグインマーケット", "Tienda de complementos", "Catalogue d'extensions", "Каталог плагинов"],
  ["在浏览器里打开插件市场，下载插件包 (.zip)", "Open the plugin market in your browser to download plugin packages (.zip)", "Plugin-Markt im Browser öffnen und Plugin-Pakete (.zip) herunterladen", "ブラウザでプラグインマーケットを開き、プラグインパッケージ (.zip) をダウンロード", "Abre la tienda de complementos en el navegador para descargar paquetes (.zip)", "Ouvrir le catalogue d'extensions dans le navigateur pour télécharger des paquets (.zip)", "Открыть каталог плагинов в браузере и скачать пакеты (.zip)"],
  ["加载插件…", "Install plugin…", "Plugin installieren…", "プラグインをインストール…", "Instalar complemento…", "Installer une extension…", "Установить плагин…"],
  ["选择下载好的插件包 (.zip) 安装或升级", "Choose a downloaded plugin package (.zip) to install or upgrade", "Heruntergeladenes Plugin-Paket (.zip) zum Installieren oder Aktualisieren wählen", "ダウンロードしたプラグインパッケージ (.zip) を選んでインストール・更新", "Elige un paquete descargado (.zip) para instalarlo o actualizarlo", "Choisir un paquet téléchargé (.zip) à installer ou mettre à jour", "Выберите скачанный пакет плагина (.zip) для установки или обновления"],
  ["卸载", "Remove", "Entfernen", "削除", "Quitar", "Supprimer", "Удалить"],
  ["删除这个插件", "Remove this plugin", "Dieses Plugin entfernen", "このプラグインを削除", "Quitar este complemento", "Supprimer cette extension", "Удалить этот плагин"],
  ["确认卸载？", "Remove?", "Entfernen?", "削除しますか？", "¿Quitar?", "Supprimer ?", "Удалить?"],
  ["已安装", "installed", "installiert", "インストール済み", "instalado", "installée", "установлен"],
  ["到插件市场下载更多插件（录制视频、蔡司 SmartSEM、蔡司 ZEN…），下载后点「加载插件…」安装",
    "Get more plugins from the plugin market (video recorder, Zeiss SmartSEM, Zeiss ZEN…), then install them with “Install plugin…”",
    "Weitere Plugins gibt es im Plugin-Markt (Videoaufnahme, Zeiss SmartSEM, Zeiss ZEN…); danach mit „Plugin installieren…“ installieren",
    "プラグインマーケットからさらにプラグイン（動画録画、Zeiss SmartSEM、Zeiss ZEN…）をダウンロードし、「プラグインをインストール…」でインストールしてください",
    "Descarga más complementos en la tienda (grabar vídeo, Zeiss SmartSEM, Zeiss ZEN…) e instálalos con «Instalar complemento…»",
    "Téléchargez d'autres extensions dans le catalogue (enregistrement vidéo, Zeiss SmartSEM, Zeiss ZEN…), puis installez-les avec « Installer une extension… »",
    "Скачайте другие плагины в каталоге (запись видео, Zeiss SmartSEM, Zeiss ZEN…) и установите их кнопкой «Установить плагин…»"],
  ["apiVersion 必须是整数", "apiVersion must be an integer", "apiVersion muss eine ganze Zahl sein", "apiVersion は整数でなければなりません", "apiVersion debe ser un número entero", "apiVersion doit être un entier", "apiVersion должен быть целым числом"],
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
  ["存图：按原始分辨率存 PNG，裁剪后只存裁剪区域，带上显示中的标注和比例尺。Ctrl/⌘+S 直接存到存图文件夹（默认是图片所在文件夹，可在「工具」里修改）",
    "Export: saves a full-resolution PNG (only the crop if cropped) with the visible annotations and scale bar. Ctrl/⌘+S saves to the save folder (the image's folder by default; change it in Tools)",
    "Export: speichert ein PNG in voller Auflösung (nur Zuschnitt, falls zugeschnitten) mit sichtbaren Beschriftungen und Maßstab. Strg/⌘+S speichert im Speicherordner (standardmäßig der Ordner des Bildes; änderbar unter Werkzeuge)",
    "書き出し：元の解像度で PNG を保存（切り抜き時はその範囲のみ）。表示中の注釈とスケールバーも含みます。Ctrl/⌘+S で保存フォルダに保存（既定は画像と同じフォルダ。「ツール」で変更可）",
    "Exportar: guarda un PNG a resolución completa (solo el recorte si lo hay) con las anotaciones y la barra de escala visibles. Ctrl/⌘+S guarda en la carpeta de guardado (por defecto, la de la imagen; se cambia en Herramientas)",
    "Exporter : enregistre un PNG en pleine résolution (seulement la zone rognée le cas échéant) avec les annotations et la barre d'échelle visibles. Ctrl/⌘+S enregistre dans le dossier d'enregistrement (par défaut celui de l'image ; modifiable dans Outils)",
    "Экспорт: сохраняет PNG в исходном разрешении (только обрезку, если она есть) с видимыми пометками и линейкой. Ctrl/⌘+S сохраняет в папку сохранения (по умолчанию папка изображения; меняется в «Инструментах»)"],
  ["显示/隐藏标注 (H)", "Show/hide annotations (H)", "Beschriftungen ein-/ausblenden (H)", "注釈の表示/非表示 (H)", "Mostrar/ocultar anotaciones (H)", "Afficher/masquer les annotations (H)", "Показать/скрыть пометки (H)"],
  ["更多：亮度对比度、标注、IMOD 模型、比例尺设置 (T)", "More: contrast, annotations, IMOD model, scale bar settings (T)", "Mehr: Kontrast, Beschriftungen, IMOD-Modell, Maßstab (T)", "その他：コントラスト、注釈、IMOD モデル、スケールバー設定 (T)", "Más: contraste, anotaciones, modelo IMOD, barra de escala (T)", "Plus : contraste, annotations, modèle IMOD, barre d'échelle (T)", "Ещё: контраст, пометки, модель IMOD, линейка (T)"],
  ["标注会在画完几秒后自动存到图片旁边的 .annot.json 文件", "Annotations are saved automatically to a .annot.json file next to the image a few seconds after drawing", "Beschriftungen werden kurz nach dem Zeichnen automatisch in einer .annot.json neben dem Bild gespeichert", "注釈は描いた数秒後に画像の隣の .annot.json に自動保存されます", "Las anotaciones se guardan solas en un .annot.json junto a la imagen unos segundos después de dibujar", "Les annotations sont enregistrées automatiquement dans un .annot.json à côté de l'image quelques secondes après le dessin", "Пометки автоматически сохраняются в .annot.json рядом с изображением через несколько секунд"],
  ["文件里没有像素尺寸，先在这里填写", "The file has no pixel size; enter it here first", "Die Datei enthält keine Pixelgröße; zuerst hier eingeben", "ファイルにピクセルサイズがありません。先にここで入力してください", "El archivo no tiene tamaño de píxel; escríbelo aquí primero", "Le fichier n'a pas de taille de pixel ; saisissez-la d'abord ici", "В файле нет размера пикселя, сначала введите его здесь"],
  ["工具：亮度对比度、标注、IMOD 模型、比例尺设置 (T)", "Tools: contrast, annotations, IMOD model, scale bar settings (T)", "Werkzeuge: Kontrast, Beschriftungen, IMOD-Modell, Maßstab (T)", "ツール：コントラスト、注釈、IMOD モデル、スケールバー設定 (T)", "Herramientas: contraste, anotaciones, modelo IMOD, barra de escala (T)", "Outils : contraste, annotations, modèle IMOD, barre d'échelle (T)", "Инструменты: контраст, пометки, модель IMOD, линейка (T)"],
  ["显示/隐藏比例尺（像素尺寸在「工具」里改）", "Show/hide scale bar (set the pixel size under Tools)", "Maßstab ein-/ausblenden (Pixelgröße unter Werkzeuge)", "スケールバーの表示/非表示（ピクセルサイズは「ツール」で変更）", "Mostrar/ocultar barra de escala (tamaño de píxel en Herramientas)", "Afficher/masquer la barre d'échelle (taille de pixel dans Outils)", "Показать/скрыть линейку (размер пикселя в «Инструментах»)"],
  ["向左", "Scroll left", "Nach links", "左へ", "Desplazar a la izquierda", "Défiler à gauche", "Влево"],
  ["向右，显示更多按钮", "Scroll right for more buttons", "Nach rechts, weitere Schaltflächen", "右へ（ほかのボタンを表示）", "Desplazar a la derecha para ver más botones", "Défiler à droite pour plus de boutons", "Вправо, больше кнопок"],
  ["↑ ↓ 切换文件　← → 切换 Z　Ctrl/⌘+滚轮 切换 Z　空格 播放　滚轮 缩放　拖动 平移　Ctrl/⌘+S 存 PNG",
    "↑ ↓ file　← → Z　Ctrl/⌘+Wheel Z　Space play　Wheel zoom　Drag pan　Ctrl/⌘+S save PNG",
    "↑ ↓ Datei　← → Z　Strg/⌘+Mausrad Z　Leertaste abspielen　Mausrad zoomen　Ziehen verschieben　Strg/⌘+S PNG speichern",
    "↑ ↓ ファイル切替　← → Z 切替　Ctrl/⌘+ホイール Z 切替　スペース 再生　ホイール ズーム　ドラッグ 移動　Ctrl/⌘+S PNG 保存",
    "↑ ↓ archivo　← → Z　Ctrl/⌘+Rueda Z　Espacio reproducir　Rueda zoom　Arrastrar desplazar　Ctrl/⌘+S guardar PNG",
    "↑ ↓ fichier　← → Z　Ctrl/⌘+Molette Z　Espace lecture　Molette zoom　Glisser déplacer　Ctrl/⌘+S PNG",
    "↑ ↓ файл　← → Z　Ctrl/⌘+колесо Z　Пробел воспроизвести　Колесо масштаб　Перетаскивание сдвиг　Ctrl/⌘+S сохранить PNG"],
  ["选择 (S)：拖动标注移动它，拖空白处移动图像；拖控制点缩放或重新测量，Delete 删除", "Select (S): drag an annotation to move it, drag empty space to pan; drag a handle to resize or re-measure, Delete removes", "Auswählen (S): Beschriftung ziehen zum Verschieben, leere Fläche ziehen zum Schwenken; Griff ziehen zum Skalieren oder Neumessen, Entf löscht", "選択 (S)：注釈をドラッグで移動、空白をドラッグで画像を移動。ハンドルをドラッグで拡大縮小・再計測、Delete で削除", "Seleccionar (S): arrastra una anotación para moverla, arrastra el fondo para desplazar; arrastra un control para escalar o volver a medir, Supr la borra", "Sélection (S) : faites glisser une annotation pour la déplacer, le fond pour déplacer l'image ; une poignée pour redimensionner ou remesurer, Suppr l'efface", "Выбор (S): тяните пометку, чтобы сдвинуть её, пустое место — чтобы сдвинуть изображение; тяните маркер для размера или новой меры, Delete удаляет"],
  ["自由", "Free", "Frei", "フリー", "Libre", "Libre", "Свободно"],
  ["框", "Box", "Rechteck", "四角", "Caja", "Cadre", "Рамка"],
  ["圆", "Circle", "Kreis", "円", "Círculo", "Cercle", "Круг"],
  ["自由画线 (P)", "Freehand (P)", "Freihand (P)", "フリーハンド (P)", "Mano alzada (P)", "Main levée (P)", "От руки (P)"],
  ["画框 (R)", "Rectangle (R)", "Rechteck (R)", "四角形 (R)", "Rectángulo (R)", "Rectangle (R)", "Прямоугольник (R)"],
  ["画圆 (E)", "Ellipse (E)", "Ellipse (E)", "楕円 (E)", "Elipse (E)", "Ellipse (E)", "Эллипс (E)"],
  ["拖动调整宽度", "Drag to resize", "Ziehen, um die Breite zu ändern", "ドラッグで幅を調整", "Arrastra para cambiar el ancho", "Glisser pour redimensionner", "Перетащите, чтобы изменить ширину"],
  ["松开鼠标打开图像", "Release to open the image", "Loslassen, um das Bild zu öffnen", "ドロップして画像を開く", "Suelta para abrir la imagen", "Relâchez pour ouvrir l'image", "Отпустите, чтобы открыть изображение"],
  ["这个标注显示在哪些 Z 层，点击修改", "Z slices this annotation shows on; click to change", "Z-Ebenen, auf denen diese Beschriftung erscheint; zum Ändern klicken", "この注釈を表示する Z スライス（クリックで変更）", "Cortes Z en los que se muestra esta anotación; haz clic para cambiar", "Coupes Z où cette annotation apparaît ; cliquez pour modifier", "Слои Z, где видна пометка; нажмите, чтобы изменить"],
  ["全部 Z", "All Z", "Alle Z", "全 Z", "Todo Z", "Tout Z", "Все Z"],
  ["仅本层", "This slice", "Nur diese Ebene", "このスライスのみ", "Solo este corte", "Cette coupe", "Только этот слой"],
  ["只在当前层显示", "Show only on the current slice", "Nur auf der aktuellen Ebene zeigen", "現在のスライスだけに表示", "Mostrar solo en el corte actual", "Afficher seulement sur la coupe actuelle", "Показывать только на текущем слое"],
  ["全部层", "All slices", "Alle Ebenen", "全スライス", "Todos los cortes", "Toutes les coupes", "Все слои"],
  ["在所有层显示", "Show on all slices", "Auf allen Ebenen zeigen", "すべてのスライスに表示", "Mostrar en todos los cortes", "Afficher sur toutes les coupes", "Показывать на всех слоях"],
  ["存图文件夹", "Save folder", "Speicherordner", "保存フォルダ", "Carpeta de guardado", "Dossier d'enregistrement", "Папка сохранения"],
  ["图片所在文件夹", "Image's folder", "Ordner des Bildes", "画像と同じフォルダ", "Carpeta de la imagen", "Dossier de l'image", "Папка изображения"],
  ["选择文件夹…", "Choose folder…", "Ordner wählen…", "フォルダを選択…", "Elegir carpeta…", "Choisir un dossier…", "Выбрать папку…"],
  ["恢复默认", "Reset to default", "Standard wiederherstellen", "既定に戻す", "Restablecer", "Rétablir par défaut", "Сбросить"],
  ["打开存图的文件夹", "Open the folder saved images go to", "Ordner mit gespeicherten Bildern öffnen", "保存先フォルダを開く", "Abrir la carpeta de guardado", "Ouvrir le dossier d'enregistrement", "Открыть папку сохранения"],
  ["打开文件夹", "Open folder", "Ordner öffnen", "フォルダを開く", "Abrir carpeta", "Ouvrir le dossier", "Открыть папку"],
  ["在访达中显示当前图像", "Show the current image in Finder", "Aktuelles Bild im Finder zeigen", "現在の画像を Finder で表示", "Mostrar la imagen actual en Finder", "Afficher l'image actuelle dans le Finder", "Показать текущее изображение в Finder"],
  ["在资源管理器中显示当前图像", "Show the current image in Explorer", "Aktuelles Bild im Explorer zeigen", "現在の画像をエクスプローラーで表示", "Mostrar la imagen actual en el Explorador", "Afficher l'image actuelle dans l'Explorateur", "Показать текущее изображение в Проводнике"],
  ["打开图像所在的文件夹", "Open the image's folder", "Ordner des Bildes öffnen", "画像のあるフォルダを開く", "Abrir la carpeta de la imagen", "Ouvrir le dossier de l'image", "Открыть папку изображения"],
  // image info / acquisition metadata
  ["图像信息", "Image info", "Bildinfo", "画像情報", "Información de la imagen", "Infos sur l'image", "Сведения об изображении"],
  ["图像信息：格式、尺寸、像素尺寸，以及插件读出的显微镜采集参数", "Image info: format, size, pixel size and the microscope settings that plugins read", "Bildinfo: Format, Größe, Pixelgröße und die von Plugins gelesenen Mikroskopparameter", "画像情報：形式、サイズ、ピクセルサイズと、プラグインが読み取った顕微鏡の撮影条件", "Información: formato, tamaño, tamaño de píxel y los parámetros del microscopio que leen los complementos", "Infos : format, taille, taille de pixel et paramètres du microscope lus par les extensions", "Сведения: формат, размер, размер пикселя и параметры микроскопа, прочитанные плагинами"],
  ["读取蔡司 SmartSEM 保存的 TIF：显示电压、工作距离、放大倍数、探测器、样品台等全部采集参数，并按校准的像素尺寸显示比例尺", "Reads TIFFs saved by Zeiss SmartSEM: shows every acquisition setting (voltage, working distance, magnification, detector, stage…) and uses the calibrated pixel size for the scale bar", "Liest von Zeiss SmartSEM gespeicherte TIFFs: zeigt alle Aufnahmeparameter (Spannung, Arbeitsabstand, Vergrößerung, Detektor, Probentisch…) und nutzt die kalibrierte Pixelgröße für den Maßstab", "Zeiss SmartSEM で保存した TIF を読み込み、加速電圧・作動距離・倍率・検出器・ステージなどすべての撮影条件を表示し、校正済みピクセルサイズでスケールバーを表示", "Lee TIFF guardados por Zeiss SmartSEM: muestra todos los parámetros de adquisición (tensión, distancia de trabajo, aumento, detector, platina…) y usa el tamaño de píxel calibrado para la barra de escala", "Lit les TIFF enregistrés par Zeiss SmartSEM : affiche tous les paramètres d'acquisition (tension, distance de travail, grandissement, détecteur, platine…) et utilise la taille de pixel calibrée pour la barre d'échelle", "Читает TIFF, сохранённые Zeiss SmartSEM: показывает все параметры съёмки (напряжение, рабочее расстояние, увеличение, детектор, столик…) и использует калиброванный размер пикселя для линейки"],
  // Zeiss ZEN plugin
  ["打开蔡司 ZEN 保存的 CZI，并显示物镜、激光、探测器、针孔、扫描和 Airyscan 处理等采集参数", "Opens CZI files saved by Zeiss ZEN and shows the objective, lasers, detectors, pinhole, scan settings, Airyscan processing and other acquisition settings", "Öffnet von Zeiss ZEN gespeicherte CZI-Dateien und zeigt Objektiv, Laser, Detektoren, Pinhole, Scan-Einstellungen, Airyscan-Verarbeitung und weitere Aufnahmeparameter", "Zeiss ZEN で保存した CZI を開き、対物レンズ・レーザー・検出器・ピンホール・スキャン・Airyscan 処理などの撮影条件を表示", "Abre archivos CZI guardados por Zeiss ZEN y muestra el objetivo, láseres, detectores, pinhole, escaneo, procesamiento Airyscan y otros parámetros de adquisición", "Ouvre les fichiers CZI enregistrés par Zeiss ZEN et affiche l'objectif, les lasers, les détecteurs, le sténopé, le balayage, le traitement Airyscan et les autres paramètres d'acquisition", "Открывает CZI, сохранённые Zeiss ZEN, и показывает объектив, лазеры, детекторы, пинхол, параметры сканирования, обработку Airyscan и другие параметры съёмки"],
  ["ZEN 图像", "ZEN image", "ZEN – Bild", "ZEN 画像", "ZEN: imagen", "ZEN : image", "ZEN: изображение"],
  ["ZEN 显微镜", "ZEN microscope", "ZEN – Mikroskop", "ZEN 顕微鏡", "ZEN: microscopio", "ZEN : microscope", "ZEN: микроскоп"],
  ["ZEN 处理", "ZEN processing", "ZEN – Verarbeitung", "ZEN 処理", "ZEN: procesamiento", "ZEN : traitement", "ZEN: обработка"],
  ["ZEN 全部参数", "ZEN all settings", "ZEN – alle Parameter", "ZEN すべてのパラメータ", "ZEN: todos los parámetros", "ZEN : tous les paramètres", "ZEN: все параметры"],
  ["软件", "Software", "Software", "ソフトウェア", "Software", "Logiciel", "Программа"],
  ["采集时间", "Acquired", "Aufgenommen", "撮影日時", "Adquisición", "Acquisition", "Время съёмки"],
  ["像素类型", "Pixel type", "Pixeltyp", "ピクセル型", "Tipo de píxel", "Type de pixel", "Тип пикселя"],
  ["位深", "Bit depth", "Bittiefe", "ビット深度", "Profundidad de bits", "Profondeur de bits", "Разрядность"],
  ["原始压缩", "Original compression", "Ursprüngliche Kompression", "元の圧縮", "Compresión original", "Compression d'origine", "Исходное сжатие"],
  ["像素尺寸 X", "Pixel size X", "Pixelgröße X", "ピクセルサイズ X", "Tamaño de píxel X", "Taille de pixel X", "Размер пикселя X"],
  ["像素尺寸 Y", "Pixel size Y", "Pixelgröße Y", "ピクセルサイズ Y", "Tamaño de píxel Y", "Taille de pixel Y", "Размер пикселя Y"],
  ["Z 步长", "Z step", "Z-Schrittweite", "Z ステップ", "Paso Z", "Pas Z", "Шаг по Z"],
  ["用户", "User", "Benutzer", "ユーザー", "Usuario", "Utilisateur", "Пользователь"],
  ["显微镜", "Microscope", "Mikroskop", "顕微鏡", "Microscopio", "Microscope", "Микроскоп"],
  ["系统", "System", "System", "システム", "Sistema", "Système", "Система"],
  ["类型", "Type", "Typ", "タイプ", "Tipo", "Type", "Тип"],
  ["名称", "Name", "Name", "名前", "Nombre", "Nom", "Имя"],
  ["物镜", "Objective", "Objektiv", "対物レンズ", "Objetivo", "Objectif", "Объектив"],
  ["放大倍数", "Magnification", "Vergrößerung", "倍率", "Aumento", "Grandissement", "Увеличение"],
  ["数值孔径 NA", "Numerical aperture NA", "Numerische Apertur NA", "開口数 NA", "Apertura numérica NA", "Ouverture numérique NA", "Числовая апертура NA"],
  ["浸没介质", "Immersion", "Immersion", "液浸", "Inmersión", "Immersion", "Иммерсия"],
  ["工作距离 (µm)", "Working distance (µm)", "Arbeitsabstand (µm)", "作動距離 (µm)", "Distancia de trabajo (µm)", "Distance de travail (µm)", "Рабочее расстояние (мкм)"],
  ["总放大倍数", "Total magnification", "Gesamtvergrößerung", "総合倍率", "Aumento total", "Grandissement total", "Общее увеличение"],
  ["荧光染料", "Fluorophore", "Fluorophor", "蛍光色素", "Fluoróforo", "Fluorophore", "Флуорофор"],
  ["激发波长 (nm)", "Excitation (nm)", "Anregung (nm)", "励起波長 (nm)", "Excitación (nm)", "Excitation (nm)", "Возбуждение (нм)"],
  ["发射波长 (nm)", "Emission (nm)", "Emission (nm)", "蛍光波長 (nm)", "Emisión (nm)", "Émission (nm)", "Эмиссия (нм)"],
  ["检测范围 (nm)", "Detection range (nm)", "Detektionsbereich (nm)", "検出範囲 (nm)", "Rango de detección (nm)", "Plage de détection (nm)", "Диапазон детекции (нм)"],
  ["采集模式", "Acquisition mode", "Aufnahmemodus", "取得モード", "Modo de adquisición", "Mode d'acquisition", "Режим съёмки"],
  ["对比方式", "Contrast method", "Kontrastverfahren", "コントラスト法", "Método de contraste", "Méthode de contraste", "Метод контраста"],
  ["照明", "Illumination", "Beleuchtung", "照明", "Iluminación", "Éclairage", "Освещение"],
  ["激光", "Laser", "Laser", "レーザー", "Láser", "Laser", "Лазер"],
  ["曝光时间 (ms)", "Exposure (ms)", "Belichtung (ms)", "露光時間 (ms)", "Exposición (ms)", "Exposition (ms)", "Экспозиция (мс)"],
  ["针孔 (AU)", "Pinhole (AU)", "Pinhole (AU)", "ピンホール (AU)", "Pinhole (AU)", "Sténopé (UA)", "Пинхол (AU)"],
  ["针孔 (µm)", "Pinhole (µm)", "Pinhole (µm)", "ピンホール (µm)", "Pinhole (µm)", "Sténopé (µm)", "Пинхол (мкм)"],
  ["探测器", "Detector", "Detektor", "検出器", "Detector", "Détecteur", "Детектор"],
  ["探测器电压 (V)", "Detector voltage (V)", "Detektorspannung (V)", "検出器電圧 (V)", "Tensión del detector (V)", "Tension du détecteur (V)", "Напряжение детектора (В)"],
  ["数字增益", "Digital gain", "Digitale Verstärkung", "デジタルゲイン", "Ganancia digital", "Gain numérique", "Цифровое усиление"],
  ["偏移", "Offset", "Offset", "オフセット", "Desplazamiento", "Décalage", "Смещение"],
  ["分档", "Binning", "Binning", "ビニング", "Binning", "Binning", "Биннинг"],
  ["像素驻留 (µs)", "Pixel dwell (µs)", "Pixelverweilzeit (µs)", "ピクセル滞在時間 (µs)", "Tiempo por píxel (µs)", "Temps par pixel (µs)", "Время на пиксель (мкс)"],
  ["帧时间 (s)", "Frame time (s)", "Bildzeit (s)", "フレーム時間 (s)", "Tiempo de fotograma (s)", "Temps d'image (s)", "Время кадра (с)"],
  ["平均次数", "Averaging", "Mittelung", "積算回数", "Promediado", "Moyennage", "Усреднение"],
  ["扫描速度", "Scan speed", "Scangeschwindigkeit", "スキャン速度", "Velocidad de escaneo", "Vitesse de balayage", "Скорость сканирования"],
  ["扫描缩放", "Scan zoom", "Scan-Zoom", "スキャンズーム", "Zoom de escaneo", "Zoom de balayage", "Зум сканирования"],
  ["扫描方向", "Scan direction", "Scanrichtung", "スキャン方向", "Dirección de escaneo", "Sens de balayage", "Направление сканирования"],
  ["Airyscan 模式", "Airyscan mode", "Airyscan-Modus", "Airyscan モード", "Modo Airyscan", "Mode Airyscan", "Режим Airyscan"],
  ["采样因子", "Sampling factor", "Abtastfaktor", "サンプリング係数", "Factor de muestreo", "Facteur d'échantillonnage", "Коэффициент выборки"],
  ["强度", "strength", "Stärke", "強度", "intensidad", "intensité", "сила"],
  ["参数估计", "parameter estimation", "Parameterschätzung", "パラメータ推定", "estimación de parámetros", "estimation des paramètres", "оценка параметров"],
  ["反卷积", "deconvolution", "Dekonvolution", "デコンボリューション", "deconvolución", "déconvolution", "деконволюция"],
  ["这个 CZI 用 JPEG XR 压缩，暂不支持；请在 ZEN 里另存为不压缩或 Zstd 压缩", "This CZI is JPEG XR compressed, which is not supported yet; in ZEN, save it uncompressed or with Zstd", "Diese CZI ist mit JPEG XR komprimiert, das noch nicht unterstützt wird; in ZEN unkomprimiert oder mit Zstd speichern", "この CZI は JPEG XR 圧縮で、まだ対応していません。ZEN で非圧縮か Zstd 圧縮で保存し直してください", "Este CZI usa compresión JPEG XR, aún no compatible; en ZEN, guárdalo sin comprimir o con Zstd", "Ce CZI est compressé en JPEG XR, non pris en charge pour l'instant ; dans ZEN, enregistrez-le sans compression ou en Zstd", "Этот CZI сжат JPEG XR, что пока не поддерживается; сохраните его в ZEN без сжатия или с Zstd"],
  ["这是多文件 CZI 的分卷，请打开第一个文件", "This is a part of a multi-file CZI; open the first file", "Dies ist ein Teil einer mehrteiligen CZI; die erste Datei öffnen", "これは分割 CZI の一部です。最初のファイルを開いてください", "Es una parte de un CZI de varios archivos; abre el primer archivo", "Ceci est une partie d'un CZI en plusieurs fichiers ; ouvrez le premier fichier", "Это часть многофайлового CZI; откройте первый файл"],
  ["搜索参数", "Search settings", "Parameter suchen", "パラメータを検索", "Buscar parámetros", "Rechercher un paramètre", "Поиск параметров"],
  ["复制全部", "Copy all", "Alle kopieren", "すべてコピー", "Copiar todo", "Tout copier", "Копировать всё"],
  ["把全部参数复制为文本", "Copy every setting as text", "Alle Parameter als Text kopieren", "すべてのパラメータをテキストでコピー", "Copiar todos los parámetros como texto", "Copier tous les paramètres en texte", "Скопировать все параметры как текст"],
  ["已复制全部参数", "Copied all settings", "Alle Parameter kopiert", "すべてのパラメータをコピーしました", "Parámetros copiados", "Paramètres copiés", "Все параметры скопированы"],
  ["没有匹配的参数", "No matching settings", "Keine passenden Parameter", "一致するパラメータはありません", "No hay parámetros que coincidan", "Aucun paramètre correspondant", "Нет подходящих параметров"],
  ["未打开图像", "No image open", "Kein Bild geöffnet", "画像が開かれていません", "No hay ninguna imagen abierta", "Aucune image ouverte", "Изображение не открыто"],
  ["文件信息", "File", "Datei", "ファイル", "Archivo", "Fichier", "Файл"],
  ["格式", "Format", "Format", "形式", "Formato", "Format", "Формат"],
  ["尺寸", "Size", "Größe", "サイズ", "Tamaño", "Taille", "Размер"],
  ["数据类型", "Data type", "Datentyp", "データ型", "Tipo de datos", "Type de données", "Тип данных"],
  ["路径", "Path", "Pfad", "パス", "Ruta", "Chemin", "Путь"],
  ["SmartSEM 主要参数", "SmartSEM key settings", "SmartSEM – wichtige Parameter", "SmartSEM 主な条件", "SmartSEM: parámetros principales", "SmartSEM : paramètres principaux", "SmartSEM: основные параметры"],
  ["全部数值参数 (AP)", "All values (AP)", "Alle Werte (AP)", "すべての数値 (AP)", "Todos los valores (AP)", "Toutes les valeurs (AP)", "Все значения (AP)"],
  ["全部状态 (DP)", "All states (DP)", "Alle Zustände (DP)", "すべての状態 (DP)", "Todos los estados (DP)", "Tous les états (DP)", "Все состояния (DP)"],
  ["系统信息 (SV)", "System (SV)", "System (SV)", "システム (SV)", "Sistema (SV)", "Système (SV)", "Система (SV)"],
  ["切换视图 XY → XZ → YZ（XZ、YZ 从侧面看，拉条切换 Y、X）", "Switch view XY → XZ → YZ (XZ and YZ are side views; the slider moves Y or X)", "Ansicht wechseln XY → XZ → YZ (XZ und YZ sind Seitenansichten; der Regler wählt Y bzw. X)", "ビュー切替 XY → XZ → YZ（XZ・YZ は横から表示、スライダーで Y・X を切替）", "Cambiar vista XY → XZ → YZ (XZ e YZ son vistas laterales; el control desliza Y o X)", "Changer de vue XY → XZ → YZ (XZ et YZ sont des vues de côté ; le curseur change Y ou X)", "Смена вида XY → XZ → YZ (XZ и YZ — виды сбоку; ползунок меняет Y или X)"],
  ["XY 视图：正常看每一层", "XY view: look at each slice normally", "XY-Ansicht: jede Ebene normal ansehen", "XY ビュー：各スライスを通常表示", "Vista XY: ver cada corte normalmente", "Vue XY : voir chaque coupe normalement", "Вид XY: обычный просмотр слоёв"],
  ["XZ 视图：从侧面看，拉条切换 Y", "XZ view: side view, the slider moves Y", "XZ-Ansicht: Seitenansicht, Schieberegler wählt Y", "XZ ビュー：横から表示、スライダーで Y を切替", "Vista XZ: vista lateral, el control desliza Y", "Vue XZ : vue de côté, le curseur change Y", "Вид XZ: сбоку, ползунок меняет Y"],
  ["YZ 视图：从侧面看，拉条切换 X", "YZ view: side view, the slider moves X", "YZ-Ansicht: Seitenansicht, Schieberegler wählt X", "YZ ビュー：横から表示、スライダーで X を切替", "Vista YZ: vista lateral, el control desliza X", "Vue YZ : vue de côté, le curseur change X", "Вид YZ: сбоку, ползунок меняет X"],
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
  // image picker: "2/8  name"; the name is translated on its own
  [/^(\d+\/\d+)  (.+)$/, "$1  $2", "$1  $2", "$1  $2", "$1  $2", "$1  $2", "$1  $2"],
  [/^图像 (\d+)$/, "Image $1", "Bild $1", "画像 $1", "Imagen $1", "Image $1", "Изображение $1"],
  [/^位置 (\d+)$/, "Position $1", "Position $1", "位置 $1", "Posición $1", "Position $1", "Позиция $1"],
  [/^拼块 (\d+) · 位置 (\d+)$/, "Tile $1 · position $2", "Kachel $1 · Position $2", "タイル $1 · 位置 $2", "Mosaico $1 · posición $2", "Tuile $1 · position $2", "Плитка $1 · позиция $2"],
  // longer / more specific first
  // format plugins (WebAssembly)
  [/^需要安装「(.+)」插件才能打开 \.(\w+) 文件$/, "The “$1” plugin is needed to open .$2 files", "Zum Öffnen von .$2-Dateien wird das Plugin „$1“ benötigt", ".$2 ファイルを開くには「$1」プラグインが必要です", "Se necesita el complemento «$1» para abrir archivos .$2", "L'extension « $1 » est nécessaire pour ouvrir les fichiers .$2", "Для открытия файлов .$2 нужен плагин «$1»"],
  [/^「(.+)」插件已停用，请在「插件」里启用后再打开 \.(\w+) 文件$/, "The “$1” plugin is switched off; switch it on under Plugins to open .$2 files", "Das Plugin „$1“ ist ausgeschaltet; unter Plugins einschalten, um .$2-Dateien zu öffnen", "「$1」プラグインがオフです。.$2 ファイルを開くには「プラグイン」でオンにしてください", "El complemento «$1» está desactivado; actívalo en Complementos para abrir archivos .$2", "L'extension « $1 » est désactivée ; activez-la dans Extensions pour ouvrir les fichiers .$2", "Плагин «$1» выключен; включите его в разделе «Плагины», чтобы открыть файлы .$2"],
  [/^请把「(.+)」插件更新到 2\.0 或更新版本，才能打开 \.(\w+) 文件$/, "Update the “$1” plugin to 2.0 or newer to open .$2 files", "Das Plugin „$1“ auf 2.0 oder neuer aktualisieren, um .$2-Dateien zu öffnen", ".$2 ファイルを開くには「$1」プラグインを 2.0 以降に更新してください", "Actualiza el complemento «$1» a la 2.0 o posterior para abrir archivos .$2", "Mettez l'extension « $1 » à jour (2.0 ou plus) pour ouvrir les fichiers .$2", "Обновите плагин «$1» до версии 2.0 или новее, чтобы открыть файлы .$2"],
  [/^「(.+)」插件无法加载：(.+)$/, "The “$1” plugin could not be loaded: $2", "Das Plugin „$1“ konnte nicht geladen werden: $2", "「$1」プラグインを読み込めません：$2", "No se pudo cargar el complemento «$1»: $2", "Impossible de charger l'extension « $1 » : $2", "Не удалось загрузить плагин «$1»: $2"],
  [/^插件 (.+) 运行超时，已中止$/, "Plugin $1 took too long and was stopped", "Plugin $1 hat zu lange gebraucht und wurde abgebrochen", "プラグイン $1 が時間切れのため中止しました", "El complemento $1 tardó demasiado y se detuvo", "L'extension $1 a pris trop de temps et a été arrêtée", "Плагин $1 работал слишком долго и был остановлен"],
  [/^插件 (.+) 出错：(.+)$/, "Plugin $1 failed: $2", "Plugin $1 ist fehlgeschlagen: $2", "プラグイン $1 でエラー：$2", "Error del complemento $1: $2", "Erreur de l'extension $1 : $2", "Ошибка плагина $1: $2"],
  [/^插件 (.+) 无法打开这个文件$/, "Plugin $1 cannot open this file", "Plugin $1 kann diese Datei nicht öffnen", "プラグイン $1 はこのファイルを開けません", "El complemento $1 no puede abrir este archivo", "L'extension $1 ne peut pas ouvrir ce fichier", "Плагин $1 не может открыть этот файл"],
  [/^通道 (\d+)：(.+)$/, "Channel $1: $2", "Kanal $1: $2", "チャンネル $1：$2", "Canal $1: $2", "Canal $1 : $2", "Канал $1: $2"],
  [/^Airyscan (强度|参数估计|反卷积)（通道 (\d+)）$/, "Airyscan $1 (channel $2)", "Airyscan $1 (Kanal $2)", "Airyscan $1（チャンネル $2）", "Airyscan: $1 (canal $2)", "Airyscan : $1 (canal $2)", "Airyscan: $1 (канал $2)"],
  [/^已保存 (.+)（(\d+)×(\d+)，(\d+) 帧，(\S+) MB，用时 (\S+) 秒）$/,
    "Saved $1 ($2×$3, $4 frames, $5 MB, $6 s)", "$1 gespeichert ($2×$3, $4 Bilder, $5 MB, $6 s)", "$1 を保存しました（$2×$3、$4 フレーム、$5 MB、$6 秒）",
    "Guardado $1 ($2×$3, $4 fotogramas, $5 MB, $6 s)", "$1 enregistré ($2×$3, $4 images, $5 Mo, $6 s)", "Сохранено $1 ($2×$3, $4 кадров, $5 МБ, $6 с)"],
  [/^复制失败：(.+)$/, "Copy failed: $1", "Kopieren fehlgeschlagen: $1", "コピーに失敗しました：$1", "Error al copiar: $1", "Échec de la copie : $1", "Не удалось скопировать: $1"],
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
  [/^(.+)。再点一次切换 画笔 \/ 画框 \/ 画圆$/, "$1. Click again to switch pen / rectangle / ellipse", "$1. Erneut klicken: Stift / Rechteck / Ellipse", "$1。もう一度クリックでペン / 四角形 / 楕円を切替", "$1. Vuelve a hacer clic para cambiar lápiz / rectángulo / elipse", "$1. Cliquez à nouveau pour alterner crayon / rectangle / ellipse", "$1. Нажмите ещё раз: перо / прямоугольник / эллипс"],
  [/^已载入内存 (\d+)%$/, "In memory $1%", "Im Speicher $1 %", "メモリに読込済み $1%", "En memoria $1 %", "En mémoire $1 %", "В памяти $1%"],
  [/^正在生成 (\w+) 视图… (\d+) \/ (\d+) 层$/, "Building $1 view… $2 / $3 slices", "$1-Ansicht wird erstellt… $2 / $3 Ebenen", "$1 ビューを作成中… $2 / $3 スライス", "Creando vista $1… $2 / $3 cortes", "Création de la vue $1… $2 / $3 coupes", "Строится вид $1… $2 / $3 слоёв"],
  [/^无法切换视图：(.+)$/, "Cannot switch view: $1", "Ansicht kann nicht gewechselt werden: $1", "ビューを切り替えられません：$1", "No se puede cambiar la vista: $1", "Impossible de changer de vue : $1", "Не удаётся сменить вид: $1"],
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
  // plugin packages (plugin-host.js, src-tauri/src/plugins/package.rs)
  [/^更新到 (\S+)$/, "Update to $1", "Auf $1 aktualisieren", "$1 に更新", "Actualizar a $1", "Mettre à jour vers $1", "Обновить до $1"],
  [/^安装失败：(.+)$/, "Install failed: $1", "Installation fehlgeschlagen: $1", "インストールに失敗しました：$1", "Error al instalar: $1", "Échec de l'installation : $1", "Ошибка установки: $1"],
  [/^卸载失败：(.+)$/, "Remove failed: $1", "Entfernen fehlgeschlagen: $1", "削除に失敗しました：$1", "Error al quitar: $1", "Échec de la suppression : $1", "Ошибка удаления: $1"],
  [/^已安装 (.+)，但出错：(.+)$/, "Installed $1, but it failed: $2", "$1 installiert, aber mit Fehler: $2", "$1 をインストールしましたが、エラーが発生しました：$2", "$1 instalado, pero falló: $2", "$1 installée, mais en erreur : $2", "$1 установлен, но с ошибкой: $2"],
  [/^已安装 (.+)$/, "Installed $1", "$1 installiert", "$1 をインストールしました", "$1 instalado", "$1 installée", "Установлен $1"],
  [/^已更新 (.+)，正在重新加载…$/, "Updated $1, reloading…", "$1 aktualisiert, wird neu geladen…", "$1 を更新しました。再読み込み中…", "$1 actualizado, recargando…", "$1 mise à jour, rechargement…", "$1 обновлён, перезагрузка…"],
  [/^已卸载 (.+)$/, "Removed $1", "$1 entfernt", "$1 を削除しました", "$1 quitado", "$1 supprimée", "Удалён $1"],
  [/^无法打开插件市场：(.+)$/, "Cannot open the plugin market: $1", "Plugin-Markt kann nicht geöffnet werden: $1", "プラグインマーケットを開けません：$1", "No se puede abrir la tienda de complementos: $1", "Impossible d'ouvrir le catalogue d'extensions : $1", "Не удаётся открыть каталог плагинов: $1"],
  [/^插件 id（(.+)）和插件包（(.+)）不一致$/, "Plugin id ($1) does not match its package ($2)", "Plugin-ID ($1) passt nicht zum Paket ($2)", "プラグイン id（$1）がパッケージ（$2）と一致しません", "El id del complemento ($1) no coincide con su paquete ($2)", "L'id de l'extension ($1) ne correspond pas à son paquet ($2)", "id плагина ($1) не совпадает с пакетом ($2)"],
  [/^插件需要更新的 TifMrcView（插件接口版本 (\d+)，当前 (\d+)）$/, "The plugin needs a newer TifMrcView (plugin API $1, this one has $2)", "Das Plugin braucht ein neueres TifMrcView (Plugin-API $1, vorhanden $2)", "このプラグインには新しい TifMrcView が必要です（プラグイン API $1、現在 $2）", "El complemento necesita un TifMrcView más reciente (API $1, actual $2)", "L'extension nécessite un TifMrcView plus récent (API $1, actuelle $2)", "Плагину нужна более новая версия TifMrcView (API $1, текущий $2)"],
  [/^插件需要更新的 TifMrcView（插件接口版本 (\d+)）$/, "The plugin needs a newer TifMrcView (plugin API $1)", "Das Plugin braucht ein neueres TifMrcView (Plugin-API $1)", "このプラグインには新しい TifMrcView が必要です（プラグイン API $1）", "El complemento necesita un TifMrcView más reciente (API $1)", "L'extension nécessite un TifMrcView plus récent (API $1)", "Плагину нужна более новая версия TifMrcView (API $1)"],
  [/^插件需要 TifMrcView (\S+) 或更新版本（当前 (\S+)）$/, "The plugin needs TifMrcView $1 or newer (you have $2)", "Das Plugin braucht TifMrcView $1 oder neuer (installiert: $2)", "このプラグインには TifMrcView $1 以降が必要です（現在 $2）", "El complemento necesita TifMrcView $1 o posterior (tienes $2)", "L'extension nécessite TifMrcView $1 ou plus récent (actuelle : $2)", "Плагину нужна TifMrcView $1 или новее (текущая $2)"],
  [/^插件 (\S+) 没有安装，请先在插件市场下载安装$/, "Plugin $1 is not installed; download and install it from the plugin market first", "Plugin $1 ist nicht installiert; zuerst aus dem Plugin-Markt herunterladen und installieren", "プラグイン $1 がインストールされていません。先にプラグインマーケットからダウンロードしてインストールしてください", "El complemento $1 no está instalado; descárgalo e instálalo primero desde la tienda", "L'extension $1 n'est pas installée ; téléchargez-la et installez-la d'abord depuis le catalogue", "Плагин $1 не установлен; сначала скачайте и установите его из каталога"],
  [/^plugin\.json 不是有效的 JSON(: .*)?$/, "plugin.json is not valid JSON$1", "plugin.json ist kein gültiges JSON$1", "plugin.json が正しい JSON ではありません$1", "plugin.json no es un JSON válido$1", "plugin.json n'est pas un JSON valide$1", "plugin.json — некорректный JSON$1"],
  [/^plugin\.json 缺少 (\S+)$/, "plugin.json has no $1", "plugin.json fehlt $1", "plugin.json に $1 がありません", "a plugin.json le falta $1", "plugin.json n'a pas de $1", "в plugin.json нет $1"],
  [/^读不到 plugin\.json(: .*)?$/, "Cannot read plugin.json$1", "plugin.json kann nicht gelesen werden$1", "plugin.json を読めません$1", "No se puede leer plugin.json$1", "Impossible de lire plugin.json$1", "Не удаётся прочитать plugin.json$1"],
  [/^插件 id 只能用小写字母、数字和 -：(.+)$/, "A plugin id may only use lowercase letters, digits and -: $1", "Eine Plugin-ID darf nur Kleinbuchstaben, Ziffern und - enthalten: $1", "プラグイン id には小文字・数字・- しか使えません：$1", "El id solo admite minúsculas, dígitos y -: $1", "Un id d'extension ne peut contenir que des minuscules, des chiffres et - : $1", "id плагина может содержать только строчные буквы, цифры и -: $1"],
  [/^插件 id 不对：(.+)$/, "Invalid plugin id: $1", "Ungültige Plugin-ID: $1", "プラグイン id が不正です：$1", "Id de complemento no válido: $1", "Id d'extension invalide : $1", "Неверный id плагина: $1"],
  [/^版本号格式不对：(.+)$/, "Invalid version number: $1", "Ungültige Versionsnummer: $1", "バージョン番号の形式が不正です：$1", "Número de versión no válido: $1", "Numéro de version invalide : $1", "Неверный номер версии: $1"],
  [/^main 不是插件里的 \.js 文件：(.+)$/, "main is not a .js file inside the plugin: $1", "main ist keine .js-Datei im Plugin: $1", "main がプラグイン内の .js ファイルではありません：$1", "main no es un archivo .js del complemento: $1", "main n'est pas un fichier .js de l'extension : $1", "main — не .js-файл внутри плагина: $1"],
  [/^文件夹名和插件 id（(.+)）不一致$/, "The folder name does not match the plugin id ($1)", "Ordnername passt nicht zur Plugin-ID ($1)", "フォルダ名がプラグイン id（$1）と一致しません", "El nombre de la carpeta no coincide con el id ($1)", "Le nom du dossier ne correspond pas à l'id ($1)", "Имя папки не совпадает с id плагина ($1)"],
  [/^插件包里没有 (.+)$/, "The package does not contain $1", "Das Paket enthält kein $1", "パッケージに $1 がありません", "El paquete no contiene $1", "Le paquet ne contient pas $1", "В пакете нет $1"],
  [/^插件里没有 (.+)$/, "The plugin does not contain $1", "Das Plugin enthält kein $1", "プラグインに $1 がありません", "El complemento no contiene $1", "L'extension ne contient pas $1", "В плагине нет $1"],
  [/^不是有效的 zip 插件包(: .*)?$/, "Not a valid zip plugin package$1", "Kein gültiges ZIP-Plugin-Paket$1", "正しい zip プラグインパッケージではありません$1", "No es un paquete zip válido$1", "Ce n'est pas un paquet zip valide$1", "Это не корректный zip-пакет плагина$1"],
  [/^插件包里的文件太多（(\d+) 个，最多 (\d+) 个）$/, "Too many files in the package ($1, at most $2)", "Zu viele Dateien im Paket ($1, höchstens $2)", "パッケージのファイルが多すぎます（$1 個、最大 $2 個）", "Demasiados archivos en el paquete ($1, máximo $2)", "Trop de fichiers dans le paquet ($1, au plus $2)", "Слишком много файлов в пакете ($1, максимум $2)"],
  [/^插件包解压后太大（超过 (\d+) MB）$/, "The package is too large when unpacked (over $1 MB)", "Das entpackte Paket ist zu groß (über $1 MB)", "展開後のパッケージが大きすぎます（$1 MB 超）", "El paquete descomprimido es demasiado grande (más de $1 MB)", "Le paquet décompressé est trop volumineux (plus de $1 Mo)", "Распакованный пакет слишком большой (больше $1 МБ)"],
  [/^插件包里不能有符号链接：(.+)$/, "Plugin packages cannot contain symbolic links: $1", "Plugin-Pakete dürfen keine symbolischen Links enthalten: $1", "プラグインパッケージにシンボリックリンクは使えません：$1", "Los paquetes no pueden contener enlaces simbólicos: $1", "Les paquets ne peuvent pas contenir de liens symboliques : $1", "Пакет не может содержать символические ссылки: $1"],
  [/^压缩包里有不安全的路径：(.+)$/, "Unsafe path in the archive: $1", "Unsicherer Pfad im Archiv: $1", "アーカイブに安全でないパスがあります：$1", "Ruta no segura en el archivo: $1", "Chemin dangereux dans l'archive : $1", "Небезопасный путь в архиве: $1"],
  [/^(\S+) 是内置插件，不能另外安装$/, "$1 is built in and cannot be installed separately", "$1 ist integriert und kann nicht separat installiert werden", "$1 は内蔵プラグインのため、別途インストールできません", "$1 viene integrado y no se puede instalar aparte", "$1 est intégrée et ne peut pas être installée à part", "$1 встроен и не может быть установлен отдельно"],
  [/^无法创建插件文件夹(: .*)?$/, "Cannot create the plugin folder$1", "Plugin-Ordner kann nicht angelegt werden$1", "プラグインフォルダを作成できません$1", "No se puede crear la carpeta de complementos$1", "Impossible de créer le dossier des extensions$1", "Не удаётся создать папку плагинов$1"],
  [/^无法替换旧版本（插件文件可能正被占用）(: .*)?$/, "Cannot replace the old version (its files may be in use)$1", "Alte Version kann nicht ersetzt werden (Dateien evtl. in Benutzung)$1", "古いバージョンを置き換えられません（ファイルが使用中の可能性があります）$1", "No se puede reemplazar la versión anterior (sus archivos pueden estar en uso)$1", "Impossible de remplacer l'ancienne version (fichiers peut-être utilisés)$1", "Не удаётся заменить старую версию (файлы могут быть заняты)$1"],
  [/^无法安装插件(: .*)?$/, "Cannot install the plugin$1", "Plugin kann nicht installiert werden$1", "プラグインをインストールできません$1", "No se puede instalar el complemento$1", "Impossible d'installer l'extension$1", "Не удаётся установить плагин$1"],
  [/^无法写入 (.+?)(: .*)?$/, "Cannot write $1$2", "$1 kann nicht geschrieben werden$2", "$1 に書き込めません$2", "No se puede escribir $1$2", "Impossible d'écrire $1$2", "Не удаётся записать $1$2"],
  [/^无法删除 (.+?)(: .*)?$/, "Cannot delete $1$2", "$1 kann nicht gelöscht werden$2", "$1 を削除できません$2", "No se puede eliminar $1$2", "Impossible de supprimer $1$2", "Не удаётся удалить $1$2"],
  [/^没有安装插件 (.+)$/, "Plugin $1 is not installed", "Plugin $1 ist nicht installiert", "プラグイン $1 はインストールされていません", "El complemento $1 no está instalado", "L'extension $1 n'est pas installée", "Плагин $1 не установлен"],
  [/^无法载入 (.+)$/, "Cannot load $1", "$1 kann nicht geladen werden", "$1 を読み込めません", "No se puede cargar $1", "Impossible de charger $1", "Не удаётся загрузить $1"],
  [/^(.+) 没有调用 TifMrcView\.register$/, "$1 does not call TifMrcView.register", "$1 ruft TifMrcView.register nicht auf", "$1 が TifMrcView.register を呼び出していません", "$1 no llama a TifMrcView.register", "$1 n'appelle pas TifMrcView.register", "$1 не вызывает TifMrcView.register"],
  [/^(.+) · 已安装$/, "$1 · installed", "$1 · installiert", "$1 · インストール済み", "$1 · instalado", "$1 · installée", "$1 · установлен"],
  [/^(.+) · ([^·\s]+\.js)$/, "$1 · $2", "$1 · $2", "$1 · $2", "$1 · $2", "$1 · $2", "$1 · $2"],
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
