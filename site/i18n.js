(function () {
  var NAMES = { zh: '简体中文', en: 'English', de: 'Deutsch', ja: '日本語', es: 'Español', fr: 'Français', ru: 'Русский' };
  var HTML_LANG = { zh: 'zh-CN', en: 'en', de: 'de', ja: 'ja', es: 'es', fr: 'fr', ru: 'ru' };
  var XATTR = '<code>xattr -dr com.apple.quarantine /Applications/TifMrcView.app</code>';

  var T = {
    zh: {
      def_h: '设为默认打开方式', def_lead: '安装后 TifMrcView 一般会自动成为 .tif / .tiff / .mrc 的默认程序。如果双击还是用别的软件打开，或想让 .rec、.st、.map 以及 .czi（需装蔡司插件）也用它打开，按下面操作：',
      def_mac: '<ol><li>在访达里选中一个文件（比如 .tif），按 <b>⌘ + I</b> 显示简介</li><li>在「打开方式」里选 <b>TifMrcView</b></li><li>点「<b>全部更改…</b>」，再点「继续」</li></ol><p>每种扩展名（.tiff、.mrc、.czi…）各做一次。</p>',
      def_win: '<ol><li>右键一个文件 →「<b>打开方式</b>」→「<b>选择其他应用</b>」</li><li>选 <b>TifMrcView</b>（列表里没有就点「在电脑上选择应用」找到它）</li><li>勾选「<b>始终使用此应用打开</b>」，点确定</li></ol><p>也可以在「设置 → 应用 → 默认应用」里搜索 TifMrcView，逐个扩展名设置。</p>',
      def_linux: '<ol><li>右键一个文件 →「<b>属性</b>」→「<b>打开方式</b>」</li><li>选 <b>TifMrcView</b>，点「<b>设为默认</b>」</li></ol><p>或者在终端运行：<code>xdg-mime default TifMrcView.desktop image/tiff application/x-mrc</code></p>',
      nav_features: '功能', nav_keys: '快捷键', nav_download: '下载',
      cta: '免费下载', cta_os: '下载 {os} 版', all: '所有版本', version: '版本',
      mac_sub: 'macOS 10.15 及以上', win_sub: '无需管理员权限',
      mac_tip: '首次打开若被拦截：到「系统设置 → 隐私与安全性」点「仍要打开」，或在终端运行 ' + XATTR + '。首次启动会自动设为 .tif / .tiff / .mrc 的默认打开方式。',
      win_tip: '安装时会自动关联 .tif / .tiff / .mrc。如果双击仍用别的程序打开：右键图片 →「打开方式」→「选择其他应用」→ 选 TifMrcView，勾选「始终使用此应用」。',
      linux_tip: 'AppImage 自带运行库，下载后 <code>chmod +x</code> 即可运行。首次启动会自动设为默认打开方式。',
      dl_mac: '下载 .dmg（通用）', note_mac: 'Apple 芯片与 Intel 通用',
      dl_win: '下载安装程序', note_win: 'Windows 10 / 11，64 位',
      note_deb: 'Debian / Ubuntu', note_rpm: 'Fedora / RHEL', note_appimage: '其它发行版',
      k_file: '上一个 / 下一个文件', k_z: '切换 Z 层', space: '空格', k_play: '播放 / 暂停 Z',
      or_dbl: '或双击', k_fit: '适应窗口', k_save: '保存 PNG 到当前目录', k_open: '打开文件',
      free: '免费开源', by_author: '作者'
    },
    en: {
      def_h: 'Make it the default app', def_lead: 'TifMrcView normally becomes the default for .tif / .tiff / .mrc when installed. If another app still opens them, or you want .rec, .st, .map and .czi (needs the Zeiss plugin) to open in it too:',
      def_mac: '<ol><li>Select a file (e.g. a .tif) in Finder and press <b>⌘ + I</b> (Get Info)</li><li>Under “Open with”, choose <b>TifMrcView</b></li><li>Click <b>Change All…</b>, then Continue</li></ol><p>Repeat once per extension (.tiff, .mrc, .czi…).</p>',
      def_win: '<ol><li>Right-click a file → <b>Open with</b> → <b>Choose another app</b></li><li>Pick <b>TifMrcView</b> (if it isn\'t listed, click “Choose an app on your PC” and find it)</li><li>Tick <b>Always use this app</b> and click OK</li></ol><p>Or go to Settings → Apps → Default apps, search for TifMrcView and set each extension.</p>',
      def_linux: '<ol><li>Right-click a file → <b>Properties</b> → <b>Open With</b></li><li>Select <b>TifMrcView</b> and click <b>Set as default</b></li></ol><p>Or run in a terminal: <code>xdg-mime default TifMrcView.desktop image/tiff application/x-mrc</code></p>',
      nav_features: 'Features', nav_keys: 'Shortcuts', nav_download: 'Download',
      cta: 'Free download', cta_os: 'Download for {os}', all: 'All versions', version: 'Version',
      mac_sub: 'macOS 10.15 or later', win_sub: 'No admin rights needed',
      mac_tip: 'If macOS blocks the first launch, go to System Settings → Privacy & Security and click “Open Anyway”, or run ' + XATTR + ' in Terminal.' + ' On first launch it makes itself the default app for .tif / .tiff / .mrc.',
      win_tip: 'The installer associates .tif / .tiff / .mrc for you. If another app still opens them, right-click an image → Open with → Choose another app → TifMrcView, and tick “Always use this app”.',
      linux_tip: 'The AppImage bundles its libraries: run <code>chmod +x</code> on it and start it. On first launch it sets itself as the default viewer.',
      dl_mac: 'Download .dmg (Universal)', note_mac: 'Apple silicon and Intel',
      dl_win: 'Download installer', note_win: 'Windows 10 / 11, 64-bit',
      note_deb: 'Debian / Ubuntu', note_rpm: 'Fedora / RHEL', note_appimage: 'Other distributions',
      k_file: 'Previous / next file', k_z: 'Change Z slice', space: 'Space', k_play: 'Play / pause Z',
      or_dbl: 'or double-click', k_fit: 'Fit to window', k_save: 'Save PNG to current folder', k_open: 'Open file',
      free: 'Free and open source', by_author: 'Author'
    },
    de: {
      def_h: 'Als Standard-App festlegen', def_lead: 'Nach der Installation ist TifMrcView normalerweise Standard für .tif / .tiff / .mrc. Öffnet trotzdem ein anderes Programm sie, oder sollen auch .rec, .st, .map und .czi (mit dem Zeiss-Plugin) damit öffnen:',
      def_mac: '<ol><li>Eine Datei (z. B. .tif) im Finder auswählen und <b>⌘ + I</b> drücken (Informationen)</li><li>Unter „Öffnen mit“ <b>TifMrcView</b> wählen</li><li>Auf <b>Alle ändern …</b> klicken, dann „Fortfahren“</li></ol><p>Für jede Endung (.tiff, .mrc, .czi …) einmal wiederholen.</p>',
      def_win: '<ol><li>Rechtsklick auf eine Datei → <b>Öffnen mit</b> → <b>Andere App auswählen</b></li><li><b>TifMrcView</b> wählen (fehlt es, „App auf diesem PC auswählen“ und suchen)</li><li><b>Immer diese App verwenden</b> anhaken und OK klicken</li></ol><p>Oder in Einstellungen → Apps → Standard-Apps nach TifMrcView suchen und jede Endung zuweisen.</p>',
      def_linux: '<ol><li>Rechtsklick auf eine Datei → <b>Eigenschaften</b> → <b>Öffnen mit</b></li><li><b>TifMrcView</b> wählen und <b>Als Standard festlegen</b> klicken</li></ol><p>Oder im Terminal: <code>xdg-mime default TifMrcView.desktop image/tiff application/x-mrc</code></p>',
      nav_features: 'Funktionen', nav_keys: 'Tastenkürzel', nav_download: 'Download',
      cta: 'Kostenlos herunterladen', cta_os: 'Für {os} herunterladen', all: 'Alle Versionen', version: 'Version',
      mac_sub: 'macOS 10.15 oder neuer', win_sub: 'Keine Administratorrechte nötig',
      mac_tip: 'Blockiert macOS den ersten Start, unter Systemeinstellungen → Datenschutz & Sicherheit auf „Dennoch öffnen“ klicken oder im Terminal ' + XATTR + ' ausführen.' + ' Beim ersten Start wird es Standard-App für .tif / .tiff / .mrc.',
      win_tip: 'Der Installer verknüpft .tif / .tiff / .mrc automatisch. Öffnet trotzdem ein anderes Programm sie: Rechtsklick auf ein Bild → Öffnen mit → Andere App auswählen → TifMrcView, und „Immer diese App verwenden“ anhaken.',
      linux_tip: 'Das AppImage bringt seine Bibliotheken mit: <code>chmod +x</code> ausführen und starten. Beim ersten Start wird es Standardbetrachter.',
      dl_mac: '.dmg herunterladen (Universal)', note_mac: 'Apple Silicon und Intel',
      dl_win: 'Installer herunterladen', note_win: 'Windows 10 / 11, 64-Bit',
      note_deb: 'Debian / Ubuntu', note_rpm: 'Fedora / RHEL', note_appimage: 'Andere Distributionen',
      k_file: 'Vorherige / nächste Datei', k_z: 'Z-Ebene wechseln', space: 'Leertaste', k_play: 'Z abspielen / pausieren',
      or_dbl: 'oder Doppelklick', k_fit: 'An Fenster anpassen', k_save: 'PNG im aktuellen Ordner speichern', k_open: 'Datei öffnen',
      free: 'Kostenlos und quelloffen', by_author: 'Autor'
    },
    ja: {
      def_h: '既定のアプリにする', def_lead: '通常はインストールすると TifMrcView が .tif / .tiff / .mrc の既定アプリになります。それでも別のアプリで開く場合や、.rec・.st・.map、.czi（Zeiss プラグインが必要）も TifMrcView で開きたい場合は：',
      def_mac: '<ol><li>Finder でファイル（例：.tif）を選び、<b>⌘ + I</b> で「情報を見る」を開く</li><li>「このアプリケーションで開く」で <b>TifMrcView</b> を選ぶ</li><li>「<b>すべてを変更…</b>」を押し、「続ける」を押す</li></ol><p>拡張子（.tiff、.mrc、.czi…）ごとに 1 回ずつ行います。</p>',
      def_win: '<ol><li>ファイルを右クリック →「<b>プログラムから開く</b>」→「<b>別のプログラムを選択</b>」</li><li><b>TifMrcView</b> を選ぶ（一覧にない場合は「PC でアプリを選択する」から探す）</li><li>「<b>常にこのアプリを使う</b>」にチェックして OK</li></ol><p>「設定 → アプリ → 既定のアプリ」で TifMrcView を検索し、拡張子ごとに設定することもできます。</p>',
      def_linux: '<ol><li>ファイルを右クリック →「<b>プロパティ</b>」→「<b>開き方</b>」</li><li><b>TifMrcView</b> を選び「<b>既定に設定</b>」を押す</li></ol><p>またはターミナルで：<code>xdg-mime default TifMrcView.desktop image/tiff application/x-mrc</code></p>',
      nav_features: '機能', nav_keys: 'ショートカット', nav_download: 'ダウンロード',
      cta: '無料ダウンロード', cta_os: '{os} 版をダウンロード', all: 'すべてのバージョン', version: 'バージョン',
      mac_sub: 'macOS 10.15 以降', win_sub: '管理者権限は不要',
      mac_tip: '初回起動がブロックされた場合は「システム設定 → プライバシーとセキュリティ」で「このまま開く」を押すか、ターミナルで ' + XATTR + ' を実行してください。' + ' 初回起動時に .tif / .tiff / .mrc の既定アプリに自動設定されます。',
      win_tip: 'インストール時に .tif / .tiff / .mrc が自動で関連付けられます。それでも別のアプリで開く場合は、画像を右クリック →「プログラムから開く」→「別のプログラムを選択」→ TifMrcView を選び、「常にこのアプリを使う」にチェックしてください。',
      linux_tip: 'AppImage は必要なライブラリを同梱しています。<code>chmod +x</code> してから実行してください。初回起動時に既定のビューアに設定されます。',
      dl_mac: '.dmg をダウンロード（Universal）', note_mac: 'Apple シリコン / Intel 両対応',
      dl_win: 'インストーラをダウンロード', note_win: 'Windows 10 / 11（64 ビット）',
      note_deb: 'Debian / Ubuntu', note_rpm: 'Fedora / RHEL', note_appimage: 'その他のディストリビューション',
      k_file: '前 / 次のファイル', k_z: 'Z 層を切り替え', space: 'スペース', k_play: 'Z の再生 / 一時停止',
      or_dbl: 'またはダブルクリック', k_fit: 'ウィンドウに合わせる', k_save: '現在のフォルダに PNG を保存', k_open: 'ファイルを開く',
      free: '無料・オープンソース', by_author: '作者'
    },
    es: {
      def_h: 'Usarlo como aplicación predeterminada', def_lead: 'Al instalarlo, TifMrcView suele quedar como predeterminado para .tif / .tiff / .mrc. Si otra aplicación los sigue abriendo, o quieres abrir también .rec, .st, .map y .czi (requiere el complemento Zeiss) con él:',
      def_mac: '<ol><li>Selecciona un archivo (p. ej. .tif) en el Finder y pulsa <b>⌘ + I</b> (Obtener información)</li><li>En «Abrir con», elige <b>TifMrcView</b></li><li>Pulsa <b>Cambiar todo…</b> y luego «Continuar»</li></ol><p>Repítelo una vez por extensión (.tiff, .mrc, .czi…).</p>',
      def_win: '<ol><li>Clic derecho en un archivo → <b>Abrir con</b> → <b>Elegir otra aplicación</b></li><li>Elige <b>TifMrcView</b> (si no aparece, pulsa «Elegir una aplicación en el equipo» y búscalo)</li><li>Marca <b>Usar siempre esta aplicación</b> y pulsa Aceptar</li></ol><p>También puedes ir a Configuración → Aplicaciones → Aplicaciones predeterminadas, buscar TifMrcView y asignar cada extensión.</p>',
      def_linux: '<ol><li>Clic derecho en un archivo → <b>Propiedades</b> → <b>Abrir con</b></li><li>Elige <b>TifMrcView</b> y pulsa <b>Establecer como predeterminada</b></li></ol><p>O en una terminal: <code>xdg-mime default TifMrcView.desktop image/tiff application/x-mrc</code></p>',
      nav_features: 'Funciones', nav_keys: 'Atajos', nav_download: 'Descargar',
      cta: 'Descarga gratuita', cta_os: 'Descargar para {os}', all: 'Todas las versiones', version: 'Versión',
      mac_sub: 'macOS 10.15 o posterior', win_sub: 'No requiere permisos de administrador',
      mac_tip: 'Si macOS bloquea el primer inicio, ve a Ajustes del Sistema → Privacidad y seguridad y pulsa «Abrir igualmente», o ejecuta ' + XATTR + ' en Terminal.' + ' En el primer inicio se convierte en la aplicación predeterminada para .tif / .tiff / .mrc.',
      win_tip: 'El instalador asocia .tif / .tiff / .mrc automáticamente. Si otra aplicación los sigue abriendo: clic derecho en una imagen → Abrir con → Elegir otra aplicación → TifMrcView, y marca «Usar siempre esta aplicación».',
      linux_tip: 'El AppImage incluye sus bibliotecas: ejecuta <code>chmod +x</code> y ábrelo. En el primer inicio se establece como visor predeterminado.',
      dl_mac: 'Descargar .dmg (Universal)', note_mac: 'Apple Silicon e Intel',
      dl_win: 'Descargar instalador', note_win: 'Windows 10 / 11, 64 bits',
      note_deb: 'Debian / Ubuntu', note_rpm: 'Fedora / RHEL', note_appimage: 'Otras distribuciones',
      k_file: 'Archivo anterior / siguiente', k_z: 'Cambiar corte Z', space: 'Espacio', k_play: 'Reproducir / pausar Z',
      or_dbl: 'o doble clic', k_fit: 'Ajustar a la ventana', k_save: 'Guardar PNG en la carpeta actual', k_open: 'Abrir archivo',
      free: 'Gratis y de código abierto', by_author: 'Autor'
    },
    fr: {
      def_h: 'En faire l’application par défaut', def_lead: 'À l’installation, TifMrcView devient normalement l’application par défaut pour .tif / .tiff / .mrc. Si une autre application les ouvre encore, ou pour ouvrir aussi .rec, .st, .map et .czi (avec l’extension Zeiss) :',
      def_mac: '<ol><li>Sélectionnez un fichier (par ex. .tif) dans le Finder et appuyez sur <b>⌘ + I</b> (Lire les informations)</li><li>Dans « Ouvrir avec », choisissez <b>TifMrcView</b></li><li>Cliquez sur <b>Tout modifier…</b>, puis « Continuer »</li></ol><p>À refaire une fois par extension (.tiff, .mrc, .czi…).</p>',
      def_win: '<ol><li>Clic droit sur un fichier → <b>Ouvrir avec</b> → <b>Choisir une autre application</b></li><li>Choisissez <b>TifMrcView</b> (s’il n’apparaît pas, cliquez sur « Choisir une application sur votre PC »)</li><li>Cochez <b>Toujours utiliser cette application</b> puis OK</li></ol><p>Ou dans Paramètres → Applications → Applications par défaut, cherchez TifMrcView et associez chaque extension.</p>',
      def_linux: '<ol><li>Clic droit sur un fichier → <b>Propriétés</b> → <b>Ouvrir avec</b></li><li>Choisissez <b>TifMrcView</b> puis <b>Définir par défaut</b></li></ol><p>Ou dans un terminal : <code>xdg-mime default TifMrcView.desktop image/tiff application/x-mrc</code></p>',
      nav_features: 'Fonctions', nav_keys: 'Raccourcis', nav_download: 'Télécharger',
      cta: 'Téléchargement gratuit', cta_os: 'Télécharger pour {os}', all: 'Toutes les versions', version: 'Version',
      mac_sub: 'macOS 10.15 ou ultérieur', win_sub: 'Aucun droit administrateur requis',
      mac_tip: 'Si macOS bloque le premier lancement, allez dans Réglages Système → Confidentialité et sécurité et cliquez sur « Ouvrir quand même », ou exécutez ' + XATTR + ' dans le Terminal.' + ' Au premier lancement, elle devient l’application par défaut pour .tif / .tiff / .mrc.',
      win_tip: 'L’installateur associe .tif / .tiff / .mrc automatiquement. Si une autre application les ouvre encore : clic droit sur une image → Ouvrir avec → Choisir une autre application → TifMrcView, et cochez « Toujours utiliser cette application ».',
      linux_tip: 'L’AppImage embarque ses bibliothèques : faites <code>chmod +x</code> puis lancez-la. Au premier lancement, elle devient la visionneuse par défaut.',
      dl_mac: 'Télécharger le .dmg (Universel)', note_mac: 'Puces Apple et Intel',
      dl_win: 'Télécharger l’installateur', note_win: 'Windows 10 / 11, 64 bits',
      note_deb: 'Debian / Ubuntu', note_rpm: 'Fedora / RHEL', note_appimage: 'Autres distributions',
      k_file: 'Fichier précédent / suivant', k_z: 'Changer de plan Z', space: 'Espace', k_play: 'Lire / mettre en pause Z',
      or_dbl: 'ou double-clic', k_fit: 'Ajuster à la fenêtre', k_save: 'Enregistrer en PNG dans le dossier courant', k_open: 'Ouvrir un fichier',
      free: 'Gratuit et open source', by_author: 'Auteur'
    },
    ru: {
      def_h: 'Сделать приложением по умолчанию', def_lead: 'Обычно после установки TifMrcView становится программой по умолчанию для .tif / .tiff / .mrc. Если файлы всё равно открывает другая программа или нужно открывать в нём ещё .rec, .st, .map и .czi (нужен плагин Zeiss):',
      def_mac: '<ol><li>Выберите файл (например, .tif) в Finder и нажмите <b>⌘ + I</b> («Свойства»)</li><li>В разделе «Открывать в программе» выберите <b>TifMrcView</b></li><li>Нажмите <b>Изменить всё…</b>, затем «Продолжить»</li></ol><p>Повторите для каждого расширения (.tiff, .mrc, .czi…).</p>',
      def_win: '<ol><li>Щёлкните файл правой кнопкой → <b>Открыть с помощью</b> → <b>Выбрать другое приложение</b></li><li>Выберите <b>TifMrcView</b> (если его нет в списке, нажмите «Выбрать приложение на компьютере»)</li><li>Отметьте <b>Всегда использовать это приложение</b> и нажмите ОК</li></ol><p>Или в «Параметры → Приложения → Приложения по умолчанию» найдите TifMrcView и назначьте каждое расширение.</p>',
      def_linux: '<ol><li>Щёлкните файл правой кнопкой → <b>Свойства</b> → <b>Открывать в</b></li><li>Выберите <b>TifMrcView</b> и нажмите <b>Назначить по умолчанию</b></li></ol><p>Или в терминале: <code>xdg-mime default TifMrcView.desktop image/tiff application/x-mrc</code></p>',
      nav_features: 'Возможности', nav_keys: 'Клавиши', nav_download: 'Скачать',
      cta: 'Скачать бесплатно', cta_os: 'Скачать для {os}', all: 'Все версии', version: 'Версия',
      mac_sub: 'macOS 10.15 и новее', win_sub: 'Права администратора не нужны',
      mac_tip: 'Если macOS блокирует первый запуск, откройте «Системные настройки → Конфиденциальность и безопасность» и нажмите «Всё равно открыть» или выполните в Терминале ' + XATTR + '. При первом запуске становится приложением по умолчанию для .tif / .tiff / .mrc.',
      win_tip: 'Установщик сам связывает .tif / .tiff / .mrc. Если их всё равно открывает другая программа: щёлкните изображение правой кнопкой → «Открыть с помощью» → «Выбрать другое приложение» → TifMrcView и отметьте «Всегда использовать это приложение».',
      linux_tip: 'AppImage содержит все библиотеки: выполните <code>chmod +x</code> и запустите. При первом запуске становится просмотрщиком по умолчанию.',
      dl_mac: 'Скачать .dmg (Universal)', note_mac: 'Apple Silicon и Intel',
      dl_win: 'Скачать установщик', note_win: 'Windows 10 / 11, 64 бит',
      note_deb: 'Debian / Ubuntu', note_rpm: 'Fedora / RHEL', note_appimage: 'Другие дистрибутивы',
      k_file: 'Предыдущий / следующий файл', k_z: 'Сменить слой Z', space: 'Пробел', k_play: 'Воспроизвести / пауза Z',
      or_dbl: 'или двойной щелчок', k_fit: 'По размеру окна', k_save: 'Сохранить PNG в текущую папку', k_open: 'Открыть файл',
      free: 'Бесплатно и с открытым кодом', by_author: 'Автор'
    }
  };
  // Strings for the plugin market (site/plugins/) and the teaser on the home page.
  var EXTRA = {
    zh: {
      nav_home: '首页', nav_plugins: '插件',
      pl_btn: '浏览插件',
      pl_title: '插件市场 · TifMrcView', pl_desc: '为 TifMrcView 下载扩展插件：打开各品牌光镜和电镜的专有格式并显示采集参数、录制视频等，并附带插件开发文档。',
      pl_h1: '插件市场', pl_lead: '插件给 TifMrcView 增加功能：打开各品牌光镜和电镜的专有格式并显示采集参数，把 Z / T 序列录成视频，等等。下面列出全部可用插件。点「安装」一键装进应用（也可以下载 zip 手动加载），不需要重装。',
      pl_install_h: '怎么安装',
      pl_s1: '在下面选一个插件，下载它的 <b>.zip</b> 文件（不要解压）。',
      pl_s2: '在 TifMrcView 里打开 <b>插件</b> → <b>加载插件…</b>，选择刚下载的 zip。',
      pl_s3: '插件马上生效；可以在插件列表里随时关闭或打开。',
      pl_update: '更新：再点一次 <b>安装</b>，或加载新版本的 zip 覆盖旧版。',
      pl_remove: '删除：在插件列表里点 <b>卸载</b>。',
      pl_need: '需要 TifMrcView {v} 或更高版本', pl_need_all: '各插件需要的最低应用版本见卡片。',
      pl_builtin: '内置', pl_builtin_note: '已包含在应用里，无需下载',
      pl_native: '用到内置原生支持', pl_native_tip: '这个插件的原生部分已随应用发布，所以只能在官方版本里使用。',
      pl_download: '下载 .zip', pl_by: '作者 {a}', pl_more: '详细说明', pl_ver: '版本 {v}',
      pl_fail: '暂时无法加载插件列表，请稍后刷新重试。', pl_loading: '正在加载插件列表…', pl_empty: '还没有可下载的插件。',
      pl_dev_h: '制作插件 / 开发者文档', pl_dev_note: '',
      pl_list_h: '可用插件',
      pl_install: '安装',
      pl_install_tip: '在 TifMrcView 里一键安装（应用里会先确认）',
      pl_one: '<b>一键安装：</b>装好并打开 TifMrcView 0.5 或更新版本，点插件卡片上的 <b>安装</b>；浏览器询问时允许打开 TifMrcView，再在应用里点 <b>安装</b> 确认。',
      pl_manual: '也可以手动安装 zip：',
      pl_nohandler: '没有反应？请先安装或打开 <a href="../#download">TifMrcView 0.5 以上版本</a>，或者下载 zip 后在应用里点「加载插件…」。'
    },
    en: {
      nav_home: 'Home', nav_plugins: 'Plugins',
      pl_btn: 'Browse plugins',
      pl_title: 'Plugin market · TifMrcView', pl_desc: 'Download plugins for TifMrcView: open vendor formats from light and electron microscopes with their acquisition settings, record video and more, plus the plugin developer docs.',
      pl_h1: 'Plugin market', pl_lead: 'Plugins add features to TifMrcView: open vendor formats from light and electron microscopes with their acquisition settings, record Z / T series as video, and more. Every available plugin is listed below. Install one in a click (or download the zip and load it by hand), no reinstall needed.',
      pl_install_h: 'How to install',
      pl_s1: 'Pick a plugin below and download its <b>.zip</b> (do not unzip it).',
      pl_s2: 'In TifMrcView open <b>Plugins</b> → <b>Install plugin…</b> and choose the zip.',
      pl_s3: 'It takes effect right away; switch it off or on any time in the plugin list.',
      pl_update: 'To update, click <b>Install</b> again or load the newer zip; it replaces the old version.',
      pl_remove: 'To remove, click <b>Remove</b> in the plugin list.',
      pl_need: 'Needs TifMrcView {v} or later', pl_need_all: 'Each card shows the minimum app version it needs.',
      pl_builtin: 'Built-in', pl_builtin_note: 'Already part of the app, nothing to download',
      pl_native: 'Uses built-in native support', pl_native_tip: 'The native part of this plugin ships inside the app, so it only works in official builds.',
      pl_download: 'Download .zip', pl_by: 'by {a}', pl_more: 'Details', pl_ver: 'Version {v}',
      pl_fail: 'The plugin list could not be loaded right now. Please refresh and try again.', pl_loading: 'Loading plugins…', pl_empty: 'No plugins to download yet.',
      pl_dev_h: 'Make a plugin / developer docs', pl_dev_note: '',
      pl_list_h: 'Available plugins',
      pl_install: 'Install',
      pl_install_tip: 'Install in TifMrcView with one click (the app asks first)',
      pl_one: '<b>One click:</b> with TifMrcView 0.5 or later installed and open, click <b>Install</b> on a plugin card, let the browser open TifMrcView, then confirm with <b>Install</b> in the app.',
      pl_manual: 'Or install the zip by hand:',
      pl_nohandler: 'Nothing happened? Install or open <a href="../#download">TifMrcView 0.5 or later</a> first, or download the zip and use “Install plugin…” in the app.'
    },
    de: {
      nav_home: 'Start', nav_plugins: 'Plugins',
      pl_btn: 'Plugins ansehen',
      pl_title: 'Plugin-Markt · TifMrcView', pl_desc: 'Plugins für TifMrcView herunterladen: Herstellerformate von Licht- und Elektronenmikroskopen samt Aufnahmeparametern öffnen, Videos aufnehmen und mehr, dazu die Entwicklerdoku.',
      pl_h1: 'Plugin-Markt', pl_lead: 'Plugins erweitern TifMrcView: Herstellerformate von Licht- und Elektronenmikroskopen samt Aufnahmeparametern öffnen, Z-/T-Serien als Video aufnehmen und mehr. Unten stehen alle verfügbaren Plugins. Mit einem Klick installieren (oder die ZIP laden und von Hand einspielen), ohne Neuinstallation.',
      pl_install_h: 'So installierst du es',
      pl_s1: 'Wähle unten ein Plugin und lade die <b>.zip</b> herunter (nicht entpacken).',
      pl_s2: 'Öffne in TifMrcView <b>Plugins</b> → <b>Plugin installieren…</b> und wähle die ZIP-Datei.',
      pl_s3: 'Es wirkt sofort; in der Plugin-Liste lässt es sich jederzeit aus- und einschalten.',
      pl_update: 'Zum Aktualisieren erneut auf <b>Installieren</b> klicken oder die neuere ZIP laden; sie ersetzt die alte Version.',
      pl_remove: 'Zum Entfernen in der Plugin-Liste auf <b>Entfernen</b> klicken.',
      pl_need: 'Benötigt TifMrcView {v} oder neuer', pl_need_all: 'Jede Karte nennt die minimal nötige App-Version.',
      pl_builtin: 'Integriert', pl_builtin_note: 'Schon in der App enthalten, kein Download nötig',
      pl_native: 'Nutzt integrierte native Unterstützung', pl_native_tip: 'Der native Teil dieses Plugins steckt in der App und funktioniert daher nur in offiziellen Builds.',
      pl_download: '.zip herunterladen', pl_by: 'von {a}', pl_more: 'Details', pl_ver: 'Version {v}',
      pl_fail: 'Die Plugin-Liste konnte gerade nicht geladen werden. Bitte neu laden und erneut versuchen.', pl_loading: 'Plugins werden geladen…', pl_empty: 'Noch keine Plugins zum Download.',
      pl_dev_h: 'Plugin entwickeln / Entwicklerdoku', pl_dev_note: 'Die Entwicklerdoku gibt es auf Englisch und Chinesisch.',
      pl_list_h: 'Verfügbare Plugins',
      pl_install: 'Installieren',
      pl_install_tip: 'Mit einem Klick in TifMrcView installieren (die App fragt vorher nach)',
      pl_one: '<b>Mit einem Klick:</b> TifMrcView 0.5 oder neuer installieren und öffnen, auf der Plugin-Karte <b>Installieren</b> klicken, dem Browser das Öffnen von TifMrcView erlauben und in der App mit <b>Installieren</b> bestätigen.',
      pl_manual: 'Oder die ZIP von Hand installieren:',
      pl_nohandler: 'Nichts passiert? Zuerst <a href="../#download">TifMrcView 0.5 oder neuer</a> installieren bzw. öffnen, oder die ZIP laden und in der App „Plugin installieren…“ verwenden.'
    },
    ja: {
      nav_home: 'ホーム', nav_plugins: 'プラグイン',
      pl_btn: 'プラグインを見る',
      pl_title: 'プラグインマーケット · TifMrcView', pl_desc: 'TifMrcView のプラグインをダウンロード：光学顕微鏡・電子顕微鏡の各メーカー形式を取得パラメータ付きで開く、動画録画など。開発者向けドキュメントも掲載。',
      pl_h1: 'プラグインマーケット', pl_lead: 'プラグインで TifMrcView に機能を追加できます。光学顕微鏡・電子顕微鏡の各メーカー独自形式を取得パラメータ付きで開く、Z / T シリーズを動画に録画する、など。利用できるプラグインはすべて下に並んでいます。ワンクリックでインストールでき（zip をダウンロードして手動で読み込むことも可能）、再インストールは不要です。',
      pl_install_h: 'インストール方法',
      pl_s1: '下からプラグインを選び、<b>.zip</b> をダウンロードします（解凍しないでください）。',
      pl_s2: 'TifMrcView で <b>プラグイン</b> → <b>プラグインをインストール…</b> を開き、zip を選びます。',
      pl_s3: 'すぐに有効になります。プラグイン一覧でいつでもオン／オフできます。',
      pl_update: '更新するには、もう一度 <b>インストール</b> を押すか、新しい zip を読み込むと古いバージョンが置き換わります。',
      pl_remove: '削除するには、プラグイン一覧の <b>削除</b> を押します。',
      pl_need: 'TifMrcView {v} 以降が必要', pl_need_all: '各カードに必要な最低アプリバージョンを表示しています。',
      pl_builtin: '内蔵', pl_builtin_note: 'アプリに含まれています。ダウンロード不要',
      pl_native: '内蔵のネイティブ機能を使用', pl_native_tip: 'このプラグインのネイティブ部分はアプリに同梱されているため、公式ビルドでのみ動作します。',
      pl_download: '.zip をダウンロード', pl_by: '作者 {a}', pl_more: '詳細', pl_ver: 'バージョン {v}',
      pl_fail: 'プラグイン一覧を読み込めませんでした。再読み込みしてもう一度お試しください。', pl_loading: 'プラグインを読み込み中…', pl_empty: 'ダウンロードできるプラグインはまだありません。',
      pl_dev_h: 'プラグインを作る / 開発者向けドキュメント', pl_dev_note: '開発者向けドキュメントは英語と中国語です。',
      pl_list_h: '利用できるプラグイン',
      pl_install: 'インストール',
      pl_install_tip: 'TifMrcView にワンクリックでインストール（アプリで確認があります）',
      pl_one: '<b>ワンクリック：</b>TifMrcView 0.5 以降をインストールして開いておき、プラグインカードの <b>インストール</b> を押します。ブラウザに TifMrcView を開くよう求められたら許可し、アプリで <b>インストール</b> を押して確定します。',
      pl_manual: 'zip を手動でインストールすることもできます：',
      pl_nohandler: '反応がない場合は、先に <a href="../#download">TifMrcView 0.5 以降</a>をインストールするか開いてください。または zip をダウンロードし、アプリの「プラグインをインストール…」を使ってください。'
    },
    es: {
      nav_home: 'Inicio', nav_plugins: 'Complementos',
      pl_btn: 'Ver complementos',
      pl_title: 'Mercado de complementos · TifMrcView', pl_desc: 'Descarga complementos para TifMrcView: abre formatos de fabricantes de microscopía óptica y electrónica con sus parámetros de adquisición, graba vídeo y más, con la documentación para desarrolladores.',
      pl_h1: 'Mercado de complementos', pl_lead: 'Los complementos añaden funciones a TifMrcView: abrir formatos de fabricantes de microscopía óptica y electrónica con sus parámetros de adquisición, grabar series Z / T en vídeo y más. Abajo están todos los complementos disponibles. Instálalos con un clic (o descarga el zip y cárgalo a mano), sin reinstalar.',
      pl_install_h: 'Cómo instalar',
      pl_s1: 'Elige un complemento abajo y descarga su <b>.zip</b> (no lo descomprimas).',
      pl_s2: 'En TifMrcView abre <b>Complementos</b> → <b>Instalar complemento…</b> y elige el zip.',
      pl_s3: 'Funciona de inmediato; puedes desactivarlo o activarlo cuando quieras en la lista.',
      pl_update: 'Para actualizar, pulsa <b>Instalar</b> otra vez o carga el zip más nuevo: reemplaza la versión anterior.',
      pl_remove: 'Para quitarlo, pulsa <b>Quitar</b> en la lista de complementos.',
      pl_need: 'Requiere TifMrcView {v} o posterior', pl_need_all: 'Cada tarjeta indica la versión mínima de la app.',
      pl_builtin: 'Integrado', pl_builtin_note: 'Ya forma parte de la app, no hay nada que descargar',
      pl_native: 'Usa soporte nativo integrado', pl_native_tip: 'La parte nativa de este complemento viene dentro de la app, por lo que solo funciona en las versiones oficiales.',
      pl_download: 'Descargar .zip', pl_by: 'por {a}', pl_more: 'Detalles', pl_ver: 'Versión {v}',
      pl_fail: 'No se pudo cargar la lista de complementos. Actualiza la página e inténtalo de nuevo.', pl_loading: 'Cargando complementos…', pl_empty: 'Aún no hay complementos para descargar.',
      pl_dev_h: 'Crear un complemento / documentación para desarrolladores', pl_dev_note: 'La documentación para desarrolladores está en inglés y chino.',
      pl_list_h: 'Complementos disponibles',
      pl_install: 'Instalar',
      pl_install_tip: 'Instalar en TifMrcView con un clic (la app pide confirmación)',
      pl_one: '<b>Con un clic:</b> con TifMrcView 0.5 o posterior instalado y abierto, pulsa <b>Instalar</b> en la tarjeta del complemento, deja que el navegador abra TifMrcView y confirma con <b>Instalar</b> en la app.',
      pl_manual: 'O instala el zip a mano:',
      pl_nohandler: '¿No pasa nada? Instala o abre primero <a href="../#download">TifMrcView 0.5 o posterior</a>, o descarga el zip y usa «Instalar complemento…» en la app.'
    },
    fr: {
      nav_home: 'Accueil', nav_plugins: 'Extensions',
      pl_btn: 'Voir les extensions',
      pl_title: 'Marché des extensions · TifMrcView', pl_desc: 'Téléchargez des extensions pour TifMrcView : ouvrir les formats constructeurs de microscopie optique et électronique avec leurs paramètres d’acquisition, enregistrer des vidéos et plus, avec la documentation développeur.',
      pl_h1: 'Marché des extensions', pl_lead: 'Les extensions ajoutent des fonctions à TifMrcView : ouvrir les formats constructeurs de microscopie optique et électronique avec leurs paramètres d’acquisition, enregistrer des séries Z / T en vidéo, et plus. Toutes les extensions disponibles sont listées ci-dessous. Installez-les en un clic (ou téléchargez le zip et chargez-le à la main), sans réinstaller.',
      pl_install_h: 'Comment installer',
      pl_s1: 'Choisissez une extension ci-dessous et téléchargez son <b>.zip</b> (ne le décompressez pas).',
      pl_s2: 'Dans TifMrcView, ouvrez <b>Extensions</b> → <b>Installer une extension…</b> et choisissez le zip.',
      pl_s3: 'Elle est active immédiatement ; vous pouvez la désactiver ou la réactiver à tout moment dans la liste.',
      pl_update: 'Pour mettre à jour, cliquez à nouveau sur <b>Installer</b> ou chargez le zip plus récent : il remplace l’ancienne version.',
      pl_remove: 'Pour la supprimer, cliquez sur <b>Supprimer</b> dans la liste des extensions.',
      pl_need: 'Nécessite TifMrcView {v} ou plus récent', pl_need_all: 'Chaque carte indique la version minimale de l’appli.',
      pl_builtin: 'Intégré', pl_builtin_note: 'Déjà inclus dans l’appli, rien à télécharger',
      pl_native: 'Utilise le support natif intégré', pl_native_tip: 'La partie native de cette extension est livrée dans l’appli ; elle ne fonctionne donc que dans les versions officielles.',
      pl_download: 'Télécharger le .zip', pl_by: 'par {a}', pl_more: 'Détails', pl_ver: 'Version {v}',
      pl_fail: 'Impossible de charger la liste des extensions pour le moment. Actualisez la page et réessayez.', pl_loading: 'Chargement des extensions…', pl_empty: 'Aucune extension à télécharger pour l’instant.',
      pl_dev_h: 'Créer une extension / documentation développeur', pl_dev_note: 'La documentation développeur est en anglais et en chinois.',
      pl_list_h: 'Extensions disponibles',
      pl_install: 'Installer',
      pl_install_tip: 'Installer dans TifMrcView en un clic (l’appli demande confirmation)',
      pl_one: '<b>En un clic :</b> avec TifMrcView 0.5 ou plus récent installé et ouvert, cliquez sur <b>Installer</b> sur la fiche de l’extension, laissez le navigateur ouvrir TifMrcView, puis confirmez avec <b>Installer</b> dans l’appli.',
      pl_manual: 'Ou installez le zip à la main :',
      pl_nohandler: 'Rien ne se passe ? Installez ou ouvrez d’abord <a href="../#download">TifMrcView 0.5 ou plus récent</a>, ou téléchargez le zip et utilisez « Installer une extension… » dans l’appli.'
    },
    ru: {
      nav_home: 'Главная', nav_plugins: 'Плагины',
      pl_btn: 'Смотреть плагины',
      pl_title: 'Каталог плагинов · TifMrcView', pl_desc: 'Скачивайте плагины для TifMrcView: открытие форматов производителей световых и электронных микроскопов с параметрами съёмки, запись видео и другое, а также документация для разработчиков.',
      pl_h1: 'Каталог плагинов', pl_lead: 'Плагины добавляют TifMrcView новые возможности: открытие форматов производителей световых и электронных микроскопов с параметрами съёмки, запись Z/T-серий в видео и другое. Ниже перечислены все доступные плагины. Устанавливайте в один клик (или скачайте zip и загрузите вручную), без переустановки.',
      pl_install_h: 'Как установить',
      pl_s1: 'Выберите плагин ниже и скачайте его <b>.zip</b> (не распаковывайте).',
      pl_s2: 'В TifMrcView откройте <b>Плагины</b> → <b>Установить плагин…</b> и выберите zip.',
      pl_s3: 'Он начнёт работать сразу; в списке плагинов его можно в любой момент выключить и включить.',
      pl_update: 'Чтобы обновить, снова нажмите <b>Установить</b> или загрузите более новый zip: он заменит старую версию.',
      pl_remove: 'Чтобы удалить, нажмите <b>Удалить</b> в списке плагинов.',
      pl_need: 'Нужен TifMrcView {v} или новее', pl_need_all: 'На каждой карточке указана минимальная версия приложения.',
      pl_builtin: 'Встроен', pl_builtin_note: 'Уже входит в приложение, скачивать не нужно',
      pl_native: 'Использует встроенную нативную поддержку', pl_native_tip: 'Нативная часть этого плагина поставляется внутри приложения, поэтому он работает только в официальных сборках.',
      pl_download: 'Скачать .zip', pl_by: 'автор {a}', pl_more: 'Подробнее', pl_ver: 'Версия {v}',
      pl_fail: 'Сейчас не удалось загрузить список плагинов. Обновите страницу и повторите попытку.', pl_loading: 'Загрузка плагинов…', pl_empty: 'Пока нет плагинов для скачивания.',
      pl_dev_h: 'Создать плагин / документация разработчика', pl_dev_note: 'Документация для разработчиков доступна на английском и китайском.',
      pl_list_h: 'Доступные плагины',
      pl_install: 'Установить',
      pl_install_tip: 'Установить в TifMrcView в один клик (приложение сначала спросит)',
      pl_one: '<b>В один клик:</b> установите и откройте TifMrcView 0.5 или новее, нажмите <b>Установить</b> на карточке плагина, разрешите браузеру открыть TifMrcView и подтвердите кнопкой <b>Установить</b> в приложении.',
      pl_manual: 'Или установите zip вручную:',
      pl_nohandler: 'Ничего не произошло? Сначала установите или откройте <a href="../#download">TifMrcView 0.5 или новее</a>, либо скачайте zip и используйте «Установить плагин…» в приложении.'
    }
  };
  Object.keys(EXTRA).forEach(function (c) { for (var k in EXTRA[c]) T[c][k] = EXTRA[c][k]; });
  // Home page: hero, chapters, numbers, formats, plugins, download, star.
  // Merged after EXTRA. Headlines wrap their phrases in <span> so a line only
  // breaks between phrases (see .h span in site.css).
  var HOME = {
    zh: {
      title: 'TifMrcView · 显微与电镜图像查看器',
      desc: 'TifMrcView：又小、又快、又方便的显微与电镜图像查看器，免费开源。TIFF / OME-TIFF、冷冻电镜 MRC，数 GB 的栈双击即开；XY / XZ / YZ 三视角，按物理单位测量，多通道荧光；插件可加入新格式，例如蔡司 CZI。支持 macOS、Windows、Linux 和 Android。',
      h1: '<span>看 TIFF 和 MRC，</span><span>就该这么快。</span>',
      slogan: '又小、又快、又方便。免费开源。',
      lead: '显微与电镜图像查看器：TIFF、OME‑TIFF、MRC，数 GB 的栈双击即开。装上插件，还能打开蔡司、徕卡、尼康、奥林巴斯的共聚焦格式（CZI、LSM、LIF、ND2、OIR）和 Imaris IMS，以及 Gatan、赛默飞、TVIPS 的电镜格式（DM3 / DM4、SER、EMD、EER），并读出采集参数。macOS、Windows、Linux 通用<span class="an">，也有 Android 版</span>。',
      an_a: '下载安卓版 APK', pro_line: '分割和三维查看：<b>TifMrcView Pro</b> →',
      dl_apk: '下载 APK', note_apk: 'Android 7 及以上，arm64',
      an_tip: '手机和平板都能用的基础版查看器（不含 Pro 功能）。安装时按提示允许「安装未知应用」。双指缩放，长按标注可删除。',
      demo_try: '点击试用：拖动 Z、测距、调亮度和对比度', demo_full: '全屏打开', demo_title: 'TifMrcView 在线示例',
      demo_note: '真实界面，在网页里运行。示例是缩小过的蔡司 Airyscan 荧光图像。',
      a_hero: 'TifMrcView 主窗口：冷冻电镜断层 MRC 文件，测得线段长度 864.2 nm，左侧为对比度与标注面板，右侧为同目录缩略图',
      c1_eye: '冷冻电镜 · MRC',
      c1_h: '<span>GB 级断层，</span><span>拖动即达。</span>',
      c1_p: '文件直接映射进内存，多核并行解码，离当前层越近的，越先在后台备好。拖动 Z 拉条，画面跟手。',
      c1_s1: '每个断层文件', c1_s2: 'Z 层，拖到哪里看到哪里',
      c1_note: '图中数据：1024 × 1024 × 512，float32，15.244 Å/px。',
      a_c1: '一个 512 层的冷冻电镜断层停在第 277 层，右侧缩略图栏列出同目录中每个 2.0 GB 的 MRC 文件',
      c2_eye: 'XY · XZ · YZ',
      c2_h: '<span>换个角度，</span><span>看清结构。</span>',
      c2_p: '一键切到侧视图，沿 Z 轴剖开整个体积。在任何切面上画框，宽与高直接标在图上。',
      c2_note: 'XZ 侧视图 · 1.35 µm × 0.252 µm',
      a_c2: '同一断层的 XZ 侧视图，框选区域标注 1.35 µm × 0.252 µm，并带 200 nm 比例尺',
      c3_eye: '测量与标注',
      c3_h: '<span>每一笔，</span><span>都是真实尺寸。</span>',
      c3_p: '线、框、圆与自由画笔，按像素尺寸换算成 nm、µm。',
      c3_l1: '比例尺自动叠加，导出的 PNG 里原样保留',
      c3_l2: '同名的 IMOD 模型（.mod / .fid）自动叠加',
      c3_l3: '标注按 Z 层记录，画完自动保存',
      c3_l4: '绘制、移动、删除、裁剪，都能撤销',
      a_c3: '标注面板：自由、框、圆三种画笔，颜色与线宽，IMOD 模型与比例尺设置；右侧图像上测得 864.2 nm',
      c4_eye: '荧光 · 多通道',
      c4_h: '<span>每个通道，</span><span>恰到好处。</span>',
      c4_p: '逐个通道开关、换色、单独调对比度，每一层自动计算显示范围。OME-TIFF 与 ImageJ hyperstack 原生读取；装上对应的插件，就能打开蔡司（CZI、LSM）、徕卡（LIF、LOF）、尼康（ND2）、奥林巴斯（OIB、OIR）的共聚焦文件和 Imaris IMS，物镜、激光、探测器等参数随图显示。',
      c4_t1: '多通道 TIFF', c4_t2: '蔡司 CZI · Airyscan',
      c4_n1: '4072 × 4072 · 三通道 · C1 / C2 / C3 分别开关',
      c4_n2: 'Airyscan Z 栈 · 41 层 · 16 位 · 测得 37.8 µm',
      a_c4a: 'TifMrcView 显示三通道荧光 TIFF，工具栏中可分别勾选 C1、C2、C3',
      a_c4b: 'TifMrcView 打开蔡司 CZI Airyscan Z 栈荧光图像，带比例尺并测得 37.8 µm',
      n_h: '<span>小巧，</span><span>却从不将就。</span>',
      n_win: 'Windows 安装包', n_mac: 'macOS 通用版，Apple 芯片与 Intel',
      n_os: '平台，均为原生安装包', n_lang: '界面语言，默认跟随系统',
      fm_h: '<span>16 种格式，</span><span>双击即开。</span>',
      fm_p: '超过 4 GB 的 ImageJ 栈、MRC 的 8 种数据模式，一样从容；蔡司、徕卡、尼康、奥林巴斯的共聚焦格式、Imaris IMS、Gatan DM3 / DM4、赛默飞 SER / EMD / EER 和 TVIPS 电镜图像，以及 SmartSEM 采集参数由插件提供。',
      fm_plug: '插件',
      mo_h: '还有更多。',
      mo1t: '文件夹缩略图', mo1d: '同目录的图像按格式分组排在侧栏，非电镜格式默认折叠，↑ ↓ 切换文件。',
      mo2t: '逐层自动对比度', mo2d: '默认 0.1% 饱和自动拉伸，也可以在直方图上直接拖选。',
      mo3t: 'Z / T 播放', mo3d: '空格播放 Z 层；带时间维度的数据另有 T 拉条。',
      mo4t: '裁剪与导出', mo4d: '框出局部单独查看，按原始分辨率导出 PNG。',
      mo5t: '图像信息', mo5d: '格式、尺寸、像素尺寸和采集参数，一处查看，可搜索、可复制。',
      mo6t: '自动更新', mo6d: '新版本一到就提示，一键下载、校验签名并重启。',
      pl_eye: '插件',
      pl_h2: '<span>无限扩展。</span>',
      pl_p: '需要什么，装什么。新的文件格式也能用插件加入：在 WebAssembly 沙箱里运行，一个 zip 三大系统通用，读取速度和内置格式一样快。',
      pg1t: '录制视频', pg1d: '把 Z、T 序列录成 MP4，对比度、通道、标注和比例尺原样保留。',
      pg2d: '一个插件包含蔡司的全部格式：ZEN 的 CZI、ZEN black 的 LSM，以及 SmartSEM 电镜参数和校准比例尺。',
      pg3d: '每个品牌一个插件：LIF / LOF、ND2、OIB / OIR 和 IMS，物镜、激光、探测器等参数随图显示。',
      pg4d: 'Gatan、赛默飞、TVIPS 电镜格式：DM3 / DM4、SER、Velox EMD、EER，读出电压、放大倍数和像素尺寸。',
      dl_h: '<span>下载 TifMrcView。</span>',
      dl_p: '免费。macOS、Windows、Linux 原生安装包<span class="an">，以及 Android APK</span>。',
      k_tool: '选择 · 移动 · 测量 · 画笔 · 框 · 圆 · 裁剪', k_undo: '撤销 / 重做', k_annot: '显示 / 隐藏标注',
      st_h: '<span>开源，</span><span>免费。</span>',
      st_p: '喜欢的话，在 GitHub 上给个 ⭐。这是对作者最好的鼓励，也能让更多人找到它。',
      star_btn: '在 GitHub 上 Star', star_foot: '⭐ Star', sc_close: '关闭'
    },
    en: {
      title: 'TifMrcView · Microscopy & EM image viewer',
      desc: 'TifMrcView: a small, fast and convenient viewer for microscopy and electron microscopy, free and open source. TIFF / OME-TIFF and cryo-EM MRC stacks of many gigabytes open on double-click; XY / XZ / YZ views, measurements in physical units, multichannel fluorescence; plugins add new formats such as Zeiss CZI. For macOS, Windows, Linux and Android.',
      h1: '<span>TIFF and MRC,</span> <span>as fast as they should be.</span>',
      slogan: 'Small. Fast. Effortless. Free and open source.',
      lead: 'A viewer for microscopy and EM images: TIFF, OME‑TIFF and MRC stacks of many gigabytes open with a double-click. Add plugins to open confocal files from Zeiss, Leica, Nikon and Olympus (CZI, LSM, LIF, ND2, OIR) and Imaris IMS, EM files from Gatan, Thermo Fisher and TVIPS (DM3 / DM4, SER, EMD, EER), and to read their acquisition settings. On macOS, Windows and Linux<span class="an">, and Android</span>.',
      an_a: 'Download the Android APK', pro_line: 'Segmentation and 3D viewing: <b>TifMrcView Pro</b> →',
      dl_apk: 'Download APK', note_apk: 'Android 7+, arm64',
      an_tip: 'The basic viewer for phones and tablets (no Pro features). Allow “Install unknown apps” when asked. Pinch to zoom; long-press an annotation to delete it.',
      demo_try: 'Click to try: drag Z, measure, adjust brightness and contrast', demo_full: 'Open full screen', demo_title: 'TifMrcView live demo',
      demo_note: 'The real interface, running in your browser. The samples are downsized Zeiss Airyscan fluorescence images.',
      a_hero: 'TifMrcView main window showing a cryo-electron tomogram (MRC) with a measured line of 864.2 nm, the contrast and annotation panel on the left and folder thumbnails on the right',
      c1_eye: 'Cryo-ET · MRC',
      c1_h: '<span>Gigabyte tomograms.</span> <span>Zero wait.</span>',
      c1_p: 'Files are memory-mapped and decoded on every core, and the slices nearest to you are prepared in the background first. Drag the Z slider and the image keeps up.',
      c1_s1: 'Size of each tomogram', c1_s2: 'Z slices, live as you drag',
      c1_note: 'Data shown: 1024 × 1024 × 512, float32, 15.244 Å/px.',
      a_c1: 'A 512-slice cryo-electron tomogram at slice 277, with the thumbnail strip listing 2.0 GB MRC files from the same folder',
      c2_eye: 'XY · XZ · YZ',
      c2_h: '<span>Turn the volume.</span> <span>See the structure.</span>',
      c2_p: 'Switch to a side view in one click and cut through the whole stack along Z. Draw a box on any plane and its width and height appear right on the image.',
      c2_note: 'XZ side view · 1.35 µm × 0.252 µm',
      a_c2: 'The same tomogram in the XZ side view with a box annotation of 1.35 µm × 0.252 µm and a 200 nm scale bar',
      c3_eye: 'Measure & annotate',
      c3_h: '<span>Every stroke,</span> <span>to scale.</span>',
      c3_p: 'Lines, boxes, circles and a free pen, converted to nm and µm from the pixel size.',
      c3_l1: 'A scale bar that stays in every PNG you export',
      c3_l2: 'IMOD models (.mod / .fid) with the same name, overlaid automatically',
      c3_l3: 'Annotations kept per Z slice and saved automatically',
      c3_l4: 'Drawing, moving, deleting and cropping can all be undone',
      a_c3: 'The annotation panel with free, box and circle pens, colour and line width, IMOD model and scale bar settings, next to an 864.2 nm measurement',
      c4_eye: 'Fluorescence · Multichannel',
      c4_h: '<span>Every channel,</span> <span>just right.</span>',
      c4_p: 'Toggle, recolour and set the contrast of each channel on its own, with an automatic display range on every slice. OME-TIFF and ImageJ hyperstacks are read natively; install a plugin to open confocal files from Zeiss (CZI, LSM), Leica (LIF, LOF), Nikon (ND2) and Olympus (OIB, OIR), plus Imaris IMS, with objective, laser and detector settings alongside the image.',
      c4_t1: 'Multichannel TIFF', c4_t2: 'Zeiss CZI · Airyscan',
      c4_n1: '4072 × 4072 · 3 channels · C1 / C2 / C3 toggles',
      c4_n2: 'Airyscan Z-stack · 41 slices · 16-bit · 37.8 µm measured',
      a_c4a: 'TifMrcView showing a three-channel fluorescence TIFF with C1, C2 and C3 checkboxes in the toolbar',
      a_c4b: 'TifMrcView showing a Zeiss CZI Airyscan Z-stack in green fluorescence with a scale bar and a 37.8 µm measurement',
      n_h: '<span>Small download.</span> <span>No compromises.</span>',
      n_win: 'Windows installer', n_mac: 'macOS Universal, Apple silicon and Intel',
      n_os: 'Platforms, all native', n_lang: 'Interface languages, following your system',
      fm_h: '<span>16 formats.</span> <span>One double-click.</span>',
      fm_p: 'Including ImageJ stacks over 4 GB and 8 MRC data modes. Confocal formats (Zeiss, Leica, Nikon, Olympus), Imaris IMS, EM images from Gatan DM3 / DM4, Thermo Fisher SER / EMD / EER and TVIPS, and SmartSEM acquisition settings come from plugins.',
      fm_plug: 'plugin',
      mo_h: 'And so much more.',
      mo1t: 'Folder thumbnails', mo1d: 'Images in the same folder line up in a side strip, grouped by format, with non-EM formats folded. ↑ ↓ to switch.',
      mo2t: 'Auto contrast per slice', mo2d: '0.1 % saturation by default, or drag the range right on the histogram.',
      mo3t: 'Z / T playback', mo3d: 'Space plays through Z; time series get a T slider too.',
      mo4t: 'Crop & export', mo4d: 'Frame a region to focus on it, then export a PNG at full resolution.',
      mo5t: 'Image info', mo5d: 'Format, size, pixel size and acquisition settings in one place, searchable and copyable.',
      mo6t: 'Automatic updates', mo6d: 'A banner tells you when a new version is ready. One click downloads it, verifies the signature and restarts.',
      pl_eye: 'Plugins',
      pl_h2: '<span>Endlessly extensible.</span>',
      pl_p: 'Install only what you need. Plugins can even add new file formats: they run in a WebAssembly sandbox, one zip works on all three systems, and they read as fast as built-in formats.',
      pg1t: 'Video recorder', pg1d: 'Records Z or T series as MP4, with your contrast, channels, annotations and scale bar.',
      pg2d: 'Every Zeiss format in one plugin: CZI from ZEN, LSM from ZEN black, and SmartSEM settings with a calibrated scale bar.',
      pg3d: 'One plugin per brand: LIF / LOF, ND2, OIB / OIR and IMS, with the objective, lasers, detectors and more beside the image.',
      pg4d: 'EM formats from Gatan, Thermo Fisher and TVIPS: DM3 / DM4, SER, Velox EMD and EER, with voltage, magnification and pixel size.',
      dl_h: '<span>Download TifMrcView.</span>',
      dl_p: 'Free. Native installers for macOS, Windows and Linux<span class="an">, plus an Android APK</span>.',
      k_tool: 'Select · move · measure · pen · box · circle · crop', k_undo: 'Undo / redo', k_annot: 'Show / hide annotations',
      st_h: '<span>Open source.</span> <span>Free.</span>',
      st_p: 'If you like it, give it a ⭐ on GitHub. It means a lot to the author and helps others find it.',
      star_btn: 'Star on GitHub', star_foot: '⭐ Star', sc_close: 'Close'
    },
    de: {
      title: 'TifMrcView · Viewer für Mikroskopie- und EM-Bilder',
      desc: 'TifMrcView: ein kleiner, schneller und einfacher Viewer für Licht- und Elektronenmikroskopie, kostenlos und quelloffen. TIFF / OME-TIFF und Kryo-EM-MRC mit vielen Gigabyte öffnen per Doppelklick; XY-/XZ-/YZ-Ansichten, Messungen in physikalischen Einheiten, Mehrkanal-Fluoreszenz; Plugins ergänzen neue Formate wie Zeiss CZI. Für macOS, Windows, Linux und Android.',
      h1: '<span>TIFF und MRC –</span> <span>endlich schnell.</span>',
      slogan: 'Klein. Schnell. Einfach. Kostenlos und Open Source.',
      lead: 'Ein Viewer für Mikroskopie- und EM-Bilder: TIFF-, OME‑TIFF- und MRC-Stapel mit vielen Gigabyte öffnen per Doppelklick. Mit Plugins öffnen Sie auch Konfokaldateien von Zeiss, Leica, Nikon und Olympus (CZI, LSM, LIF, ND2, OIR) sowie Imaris IMS und EM-Dateien von Gatan, Thermo Fisher und TVIPS (DM3 / DM4, SER, EMD, EER) und sehen deren Aufnahmeparameter. Auf macOS, Windows und Linux<span class="an"> sowie Android</span>.',
      an_a: 'Android-APK herunterladen', pro_line: 'Segmentierung und 3D-Ansicht: <b>TifMrcView Pro</b> →',
      dl_apk: 'APK herunterladen', note_apk: 'Ab Android 7, arm64',
      an_tip: 'Der Basis-Viewer für Smartphones und Tablets (ohne Pro-Funktionen). Bei Nachfrage „Unbekannte Apps installieren“ erlauben. Mit zwei Fingern zoomen; Anmerkung lange drücken zum Löschen.',
      demo_try: 'Zum Ausprobieren klicken: Z ziehen, messen, Helligkeit und Kontrast anpassen', demo_full: 'Im Vollbild öffnen', demo_title: 'TifMrcView Live-Demo',
      demo_note: 'Die echte Oberfläche, direkt im Browser. Die Beispiele sind verkleinerte Zeiss-Airyscan-Fluoreszenzbilder.',
      a_hero: 'TifMrcView-Hauptfenster mit einem Kryo-Elektronentomogramm (MRC) und einer gemessenen Linie von 864,2 nm, links der Kontrast- und Annotationsbereich, rechts die Miniaturen des Ordners',
      c1_eye: 'Kryo-ET · MRC',
      c1_h: '<span>Tomogramme in Gigabyte.</span> <span>Ohne Warten.</span>',
      c1_p: 'Dateien werden in den Speicher gemappt und auf allen Kernen dekodiert; die nächstgelegenen Ebenen liegen im Hintergrund zuerst bereit. Ziehen Sie den Z-Regler – das Bild folgt.',
      c1_s1: 'Größe jedes Tomogramms', c1_s2: 'Z-Ebenen, live beim Ziehen',
      c1_note: 'Gezeigte Daten: 1024 × 1024 × 512, float32, 15,244 Å/px.',
      a_c1: 'Ein Kryo-Elektronentomogramm mit 512 Ebenen bei Ebene 277; die Miniaturleiste zeigt MRC-Dateien mit je 2,0 GB aus demselben Ordner',
      c2_eye: 'XY · XZ · YZ',
      c2_h: '<span>Volumen drehen.</span> <span>Struktur erkennen.</span>',
      c2_p: 'Mit einem Klick in die Seitenansicht und entlang Z durch den ganzen Stapel schneiden. Zeichnen Sie auf jeder Ebene einen Rahmen – Breite und Höhe stehen direkt im Bild.',
      c2_note: 'XZ-Seitenansicht · 1,35 µm × 0,252 µm',
      a_c2: 'Dasselbe Tomogramm in der XZ-Seitenansicht mit einem Rahmen von 1,35 µm × 0,252 µm und einer 200-nm-Maßstabsleiste',
      c3_eye: 'Messen & Annotieren',
      c3_h: '<span>Jeder Strich</span> <span>maßstabsgetreu.</span>',
      c3_p: 'Linien, Rahmen, Kreise und freier Stift – aus der Pixelgröße in nm und µm umgerechnet.',
      c3_l1: 'Eine Maßstabsleiste, die in jedem exportierten PNG erhalten bleibt',
      c3_l2: 'Gleichnamige IMOD-Modelle (.mod / .fid) werden automatisch überlagert',
      c3_l3: 'Annotationen je Z-Ebene, automatisch gespeichert',
      c3_l4: 'Zeichnen, Verschieben, Löschen und Zuschneiden lassen sich rückgängig machen',
      a_c3: 'Der Annotationsbereich mit freiem Stift, Rahmen und Kreis, Farbe und Linienbreite, IMOD-Modell und Maßstab, daneben eine Messung von 864,2 nm',
      c4_eye: 'Fluoreszenz · Mehrkanal',
      c4_h: '<span>Jeder Kanal</span> <span>genau richtig.</span>',
      c4_p: 'Jeden Kanal einzeln ein- und ausblenden, umfärben und im Kontrast einstellen, mit automatischem Anzeigebereich auf jeder Ebene. OME-TIFF und ImageJ-Hyperstacks werden nativ gelesen; mit dem passenden Plugin öffnen Sie Konfokaldateien von Zeiss (CZI, LSM), Leica (LIF, LOF), Nikon (ND2) und Olympus (OIB, OIR) sowie Imaris IMS, Objektiv, Laser und Detektor stehen direkt neben dem Bild.',
      c4_t1: 'Mehrkanal-TIFF', c4_t2: 'Zeiss CZI · Airyscan',
      c4_n1: '4072 × 4072 · 3 Kanäle · C1 / C2 / C3 einzeln schaltbar',
      c4_n2: 'Airyscan-Z-Stapel · 41 Ebenen · 16 Bit · 37,8 µm gemessen',
      a_c4a: 'TifMrcView zeigt ein dreikanaliges Fluoreszenz-TIFF mit den Kontrollkästchen C1, C2 und C3 in der Werkzeugleiste',
      a_c4b: 'TifMrcView zeigt einen Zeiss-CZI-Airyscan-Z-Stapel in grüner Fluoreszenz mit Maßstab und einer Messung von 37,8 µm',
      n_h: '<span>Kleiner Download.</span> <span>Keine Kompromisse.</span>',
      n_win: 'Windows-Installer', n_mac: 'macOS Universal, Apple Silicon und Intel',
      n_os: 'Plattformen, alle nativ', n_lang: 'Sprachen der Oberfläche, passend zum System',
      fm_h: '<span>16 Formate.</span> <span>Ein Doppelklick.</span>',
      fm_p: 'Einschließlich ImageJ-Stapeln über 4 GB und 8 MRC-Datenmodi. Konfokalformate (Zeiss, Leica, Nikon, Olympus), Imaris IMS, EM-Bilder aus Gatan DM3/DM4, Thermo Fisher SER/EMD/EER und TVIPS sowie die Aufnahmeparameter von SmartSEM liefern Plugins.',
      fm_plug: 'Plugin',
      mo_h: 'Und noch viel mehr.',
      mo1t: 'Ordner-Miniaturen', mo1d: 'Bilder aus demselben Ordner in einer Seitenleiste, nach Format gruppiert; Nicht-EM-Formate sind eingeklappt. Mit ↑ ↓ wechseln.',
      mo2t: 'Autokontrast je Ebene', mo2d: 'Standardmäßig 0,1 % Sättigung – oder den Bereich direkt im Histogramm ziehen.',
      mo3t: 'Z / T abspielen', mo3d: 'Die Leertaste spielt Z ab; Zeitreihen bekommen zusätzlich einen T-Regler.',
      mo4t: 'Zuschneiden & Export', mo4d: 'Einen Bereich rahmen, nur ihn betrachten und als PNG in voller Auflösung exportieren.',
      mo5t: 'Bildinfo', mo5d: 'Format, Größe, Pixelgröße und Aufnahmeparameter an einem Ort – durchsuchbar und kopierbar.',
      mo6t: 'Automatische Updates', mo6d: 'Ein Hinweis meldet neue Versionen. Ein Klick lädt sie, prüft die Signatur und startet neu.',
      pl_eye: 'Plugins',
      pl_h2: '<span>Grenzenlos erweiterbar.</span>',
      pl_p: 'Installieren Sie nur, was Sie brauchen. Plugins können sogar neue Dateiformate ergänzen: Sie laufen in einer WebAssembly-Sandbox, ein ZIP funktioniert auf allen drei Systemen und liest so schnell wie die eingebauten Formate.',
      pg1t: 'Videorekorder', pg1d: 'Nimmt Z- oder T-Serien als MP4 auf – mit Kontrast, Kanälen, Annotationen und Maßstabsleiste.',
      pg2d: 'Alle Zeiss-Formate in einem Plugin: CZI aus ZEN, LSM aus ZEN black und SmartSEM-Parameter mit kalibriertem Maßstab.',
      pg3d: 'Ein Plugin pro Marke: LIF / LOF, ND2, OIB / OIR und IMS, mit Objektiv, Lasern, Detektoren und mehr neben dem Bild.',
      pg4d: 'EM-Formate von Gatan, Thermo Fisher und TVIPS: DM3 / DM4, SER, Velox EMD und EER, mit Spannung, Vergrößerung und Pixelgröße.',
      dl_h: '<span>TifMrcView herunterladen.</span>',
      dl_p: 'Kostenlos. Native Installer für macOS, Windows und Linux<span class="an">, dazu eine Android-APK</span>.',
      k_tool: 'Auswählen · Verschieben · Messen · Stift · Rahmen · Kreis · Zuschneiden', k_undo: 'Rückgängig / Wiederholen', k_annot: 'Annotationen ein- / ausblenden',
      st_h: '<span>Quelloffen.</span> <span>Kostenlos.</span>',
      st_p: 'Gefällt es Ihnen? Dann geben Sie ihm einen ⭐ auf GitHub. Das freut den Autor und hilft anderen, es zu finden.',
      star_btn: 'Auf GitHub mit Star markieren', star_foot: '⭐ Star', sc_close: 'Schließen'
    },
    ja: {
      title: 'TifMrcView · 顕微鏡・電子顕微鏡画像ビューア',
      desc: 'TifMrcView：顕微鏡と電子顕微鏡のための、小さく、速く、使いやすい無料のオープンソースビューア。TIFF / OME-TIFF とクライオ電顕の MRC は数 GB のスタックもダブルクリックで開きます。XY / XZ / YZ ビュー、物理単位での計測、マルチチャンネル蛍光。プラグインで Zeiss CZI などの新しい形式も追加できます。macOS・Windows・Linux・Android 対応。',
      h1: '<span>TIFF と MRC を、</span><span>もっと速く。</span>',
      slogan: '小さく、速く、使いやすく。無料のオープンソース。',
      lead: '顕微鏡・電子顕微鏡画像のビューア。TIFF、OME‑TIFF、MRC は数 GB のスタックもダブルクリックで開きます。プラグインを入れれば Zeiss・Leica・Nikon・Olympus の共焦点ファイル（CZI、LSM、LIF、ND2、OIR）や Imaris IMS、Gatan・Thermo Fisher・TVIPS の電子顕微鏡ファイル（DM3 / DM4、SER、EMD、EER）も開け、取得パラメータも読めます。macOS・Windows・Linux<span class="an">・Android</span> に対応。',
      an_a: 'Android 版 APK をダウンロード', pro_line: 'セグメンテーションと 3D 表示は <b>TifMrcView Pro</b> へ →',
      dl_apk: 'APK をダウンロード', note_apk: 'Android 7 以降、arm64',
      an_tip: 'スマートフォン・タブレット向けの基本ビューア（Pro 機能なし）。インストール時に「不明なアプリのインストール」を許可してください。2 本指でズーム、注釈を長押しで削除。',
      demo_try: 'クリックして試す：Z の移動、計測、明るさとコントラストの調整', demo_full: '全画面で開く', demo_title: 'TifMrcView ライブデモ',
      demo_note: '本物の画面をブラウザで動かしています。サンプルは縮小した Zeiss Airyscan 蛍光画像です。',
      a_hero: 'TifMrcView のメインウィンドウ。クライオ電子線トモグラム（MRC）に 864.2 nm の計測線、左にコントラストと注釈のパネル、右に同じフォルダのサムネイル',
      c1_eye: 'クライオ ET · MRC',
      c1_h: '<span>ギガバイト級のトモグラムも、</span><span>待たせない。</span>',
      c1_p: 'ファイルはメモリマップで読み込み、全コアで並列デコード。いまの層に近いスライスから、バックグラウンドで先に用意します。Z スライダーを動かせば、画像がそのままついてきます。',
      c1_s1: 'トモグラム 1 ファイルのサイズ', c1_s2: 'Z スライス、ドラッグのままに',
      c1_note: '表示データ：1024 × 1024 × 512、float32、15.244 Å/px。',
      a_c1: '512 層のクライオ電子線トモグラムの第 277 層。右のサムネイル列には同じフォルダの 2.0 GB の MRC ファイルが並ぶ',
      c2_eye: 'XY · XZ · YZ',
      c2_h: '<span>角度を変えれば、</span><span>構造が見える。</span>',
      c2_p: 'ワンクリックで側面ビューへ。Z 方向にボリューム全体を切り開きます。どの断面でも枠を描けば、幅と高さがそのまま画像に表示されます。',
      c2_note: 'XZ 側面ビュー · 1.35 µm × 0.252 µm',
      a_c2: 'XZ 側面ビューに切り替えたトモグラム。枠の範囲は 1.35 µm × 0.252 µm、200 nm のスケールバー付き',
      c3_eye: '計測と注釈',
      c3_h: '<span>描いた線は、</span><span>すべて実寸。</span>',
      c3_p: '線、枠、円、フリーハンド。ピクセルサイズから nm や µm に換算して表示します。',
      c3_l1: '書き出した PNG にもそのまま残るスケールバー',
      c3_l2: '同名の IMOD モデル（.mod / .fid）を自動で重ねて表示',
      c3_l3: '注釈は Z 層ごとに記録され、自動で保存',
      c3_l4: '描画・移動・削除・トリミングはすべて取り消し可能',
      a_c3: 'フリー・枠・円のペン、色と線幅、IMOD モデルとスケールバーの設定がある注釈パネルと、864.2 nm の計測',
      c4_eye: '蛍光 · マルチチャンネル',
      c4_h: '<span>チャンネルごとに、</span><span>ちょうどいい。</span>',
      c4_p: 'チャンネルごとに表示の切り替え、色の変更、コントラスト調整。スライスごとに表示範囲を自動で決めます。OME-TIFF と ImageJ hyperstack はネイティブに読み込み、対応するプラグインを入れれば、Zeiss（CZI、LSM）、Leica（LIF、LOF）、Nikon（ND2）、Olympus（OIB、OIR）の共焦点ファイルと Imaris IMS を開き、対物レンズ・レーザー・検出器の設定も画像と一緒に表示します。',
      c4_t1: 'マルチチャンネル TIFF', c4_t2: 'Zeiss CZI · Airyscan',
      c4_n1: '4072 × 4072 · 3 チャンネル · C1 / C2 / C3 を個別に切り替え',
      c4_n2: 'Airyscan Z スタック · 41 層 · 16 ビット · 37.8 µm を計測',
      a_c4a: 'ツールバーに C1・C2・C3 のチェックボックスがある、3 チャンネル蛍光 TIFF を表示した TifMrcView',
      a_c4b: 'Zeiss CZI Airyscan の Z スタック蛍光画像をスケールバー付きで表示し、37.8 µm を計測した TifMrcView',
      n_h: '<span>小さなダウンロード。</span><span>妥協のない中身。</span>',
      n_win: 'Windows インストーラ', n_mac: 'macOS ユニバーサル版（Apple シリコン / Intel）',
      n_os: 'プラットフォーム、すべてネイティブ', n_lang: 'UI 言語、システムに合わせて自動選択',
      fm_h: '<span>16 の形式を、</span><span>ダブルクリックで。</span>',
      fm_p: '4 GB を超える ImageJ スタック、MRC の 8 つのデータモードにも対応。共焦点形式（Zeiss・Leica・Nikon・Olympus）、Imaris IMS、Gatan DM3 / DM4・Thermo Fisher SER / EMD / EER・TVIPS の電顕画像、SmartSEM の取得パラメータはプラグインで追加できます。',
      fm_plug: 'プラグイン',
      mo_h: 'まだまだ、あります。',
      mo1t: 'フォルダのサムネイル', mo1d: '同じフォルダの画像を形式ごとにサイドバーに並べ、電顕以外の形式は折りたたみ。↑ ↓ で切り替え。',
      mo2t: 'スライスごとの自動コントラスト', mo2d: '既定は 0.1 % 飽和。ヒストグラム上で範囲を直接ドラッグすることも。',
      mo3t: 'Z / T 再生', mo3d: 'スペースで Z を再生。時系列データには T スライダーも。',
      mo4t: 'トリミングと書き出し', mo4d: '範囲を囲んでその部分だけを表示し、元の解像度で PNG に書き出し。',
      mo5t: '画像情報', mo5d: '形式、サイズ、ピクセルサイズ、取得パラメータを一か所で。検索もコピーも。',
      mo6t: '自動アップデート', mo6d: '新しいバージョンをお知らせ。ワンクリックでダウンロード、署名を検証して再起動。',
      pl_eye: 'プラグイン',
      pl_h2: '<span>どこまでも拡張。</span>',
      pl_p: '必要なものだけを入れる。新しいファイル形式もプラグインで追加できます。WebAssembly のサンドボックスで動き、1 つの zip が 3 つの OS で使え、読み込みは内蔵形式と同じ速さです。',
      pg1t: 'ビデオ録画', pg1d: 'Z や T のシリーズを MP4 に。コントラスト、チャンネル、注釈、スケールバーもそのまま。',
      pg2d: 'Zeiss の全形式を 1 つのプラグインに：ZEN の CZI、ZEN black の LSM、SmartSEM の取得条件と校正済みスケールバー。',
      pg3d: 'ブランドごとに 1 つのプラグイン：LIF / LOF、ND2、OIB / OIR、IMS。対物レンズ、レーザー、検出器などを画像と一緒に表示。',
      pg4d: 'Gatan・Thermo Fisher・TVIPS の電子顕微鏡形式：DM3 / DM4、SER、Velox EMD、EER。加速電圧、倍率、ピクセルサイズも表示。',
      dl_h: '<span>TifMrcView を</span><span>ダウンロード。</span>',
      dl_p: '無料。macOS・Windows・Linux のネイティブインストーラ<span class="an">と Android APK</span>。',
      k_tool: '選択 · 移動 · 計測 · ペン · 枠 · 円 · トリミング', k_undo: '取り消し / やり直し', k_annot: '注釈の表示 / 非表示',
      st_h: '<span>オープンソース。</span><span>無料。</span>',
      st_p: '気に入ったら GitHub で ⭐ をお願いします。作者の励みになり、ほかの人にも見つけてもらえます。',
      star_btn: 'GitHub で Star', star_foot: '⭐ Star', sc_close: '閉じる'
    },
    es: {
      title: 'TifMrcView · Visor de imágenes de microscopía y ME',
      desc: 'TifMrcView: un visor pequeño, rápido y cómodo para microscopía óptica y electrónica, gratis y de código abierto. Pilas TIFF / OME-TIFF y MRC de crio-ME de varios GB se abren con doble clic; vistas XY / XZ / YZ, medidas en unidades físicas y fluorescencia multicanal; los complementos añaden formatos nuevos como Zeiss CZI. Para macOS, Windows, Linux y Android.',
      h1: '<span>TIFF y MRC,</span> <span>tan rápido como debe ser.</span>',
      slogan: 'Ligero. Rápido. Sencillo. Gratis y de código abierto.',
      lead: 'Un visor para imágenes de microscopía óptica y electrónica: pilas TIFF, OME‑TIFF y MRC de varios gigabytes se abren con doble clic. Con complementos, también abre archivos confocales de Zeiss, Leica, Nikon y Olympus (CZI, LSM, LIF, ND2, OIR) e Imaris IMS, archivos de microscopía electrónica de Gatan, Thermo Fisher y TVIPS (DM3 / DM4, SER, EMD, EER), y lee sus parámetros de adquisición. En macOS, Windows y Linux<span class="an">, y también Android</span>.',
      an_a: 'Descargar el APK para Android', pro_line: 'Segmentación y vista 3D: <b>TifMrcView Pro</b> →',
      dl_apk: 'Descargar APK', note_apk: 'Android 7 o posterior, arm64',
      an_tip: 'El visor básico para móviles y tabletas (sin funciones Pro). Permite «Instalar apps desconocidas» cuando se pida. Pellizca para hacer zoom; mantén pulsada una anotación para borrarla.',
      demo_try: 'Haz clic para probar: mueve Z, mide, ajusta brillo y contraste', demo_full: 'Abrir a pantalla completa', demo_title: 'Demo en vivo de TifMrcView',
      demo_note: 'La interfaz real, funcionando en el navegador. Las muestras son imágenes de fluorescencia Zeiss Airyscan reducidas.',
      a_hero: 'Ventana principal de TifMrcView con un tomograma de crio-microscopía electrónica (MRC) y una línea medida de 864,2 nm, el panel de contraste y anotaciones a la izquierda y las miniaturas de la carpeta a la derecha',
      c1_eye: 'Crio-ET · MRC',
      c1_h: '<span>Tomogramas de gigabytes.</span> <span>Sin esperas.</span>',
      c1_p: 'Los archivos se mapean en memoria y se decodifican en todos los núcleos, y los cortes más cercanos se preparan primero en segundo plano. Arrastra el control Z y la imagen te sigue.',
      c1_s1: 'Tamaño de cada tomograma', c1_s2: 'Cortes Z, en directo al arrastrar',
      c1_note: 'Datos mostrados: 1024 × 1024 × 512, float32, 15,244 Å/px.',
      a_c1: 'Un tomograma de crio-ME de 512 cortes en el corte 277; la barra de miniaturas muestra archivos MRC de 2,0 GB de la misma carpeta',
      c2_eye: 'XY · XZ · YZ',
      c2_h: '<span>Gira el volumen.</span> <span>Descubre la estructura.</span>',
      c2_p: 'Pasa a una vista lateral con un clic y corta toda la pila a lo largo de Z. Dibuja un recuadro en cualquier plano y su ancho y alto aparecen sobre la imagen.',
      c2_note: 'Vista lateral XZ · 1,35 µm × 0,252 µm',
      a_c2: 'El mismo tomograma en la vista lateral XZ con un recuadro de 1,35 µm × 0,252 µm y una barra de escala de 200 nm',
      c3_eye: 'Medir y anotar',
      c3_h: '<span>Cada trazo,</span> <span>a escala.</span>',
      c3_p: 'Líneas, recuadros, círculos y lápiz libre, convertidos a nm y µm según el tamaño de píxel.',
      c3_l1: 'Una barra de escala que se conserva en cada PNG exportado',
      c3_l2: 'Modelos IMOD (.mod / .fid) con el mismo nombre, superpuestos automáticamente',
      c3_l3: 'Anotaciones por corte Z, guardadas automáticamente',
      c3_l4: 'Dibujar, mover, borrar y recortar: todo se puede deshacer',
      a_c3: 'El panel de anotaciones con lápiz libre, recuadro y círculo, color y grosor, modelo IMOD y barra de escala, junto a una medida de 864,2 nm',
      c4_eye: 'Fluorescencia · Multicanal',
      c4_h: '<span>Cada canal,</span> <span>en su punto.</span>',
      c4_p: 'Activa, colorea y ajusta el contraste de cada canal por separado, con rango de visualización automático en cada corte. OME-TIFF e hyperstacks de ImageJ se leen de forma nativa; instala el complemento correspondiente para abrir archivos confocales de Zeiss (CZI, LSM), Leica (LIF, LOF), Nikon (ND2) y Olympus (OIB, OIR), además de Imaris IMS, con el objetivo, el láser y el detector junto a la imagen.',
      c4_t1: 'TIFF multicanal', c4_t2: 'Zeiss CZI · Airyscan',
      c4_n1: '4072 × 4072 · 3 canales · C1 / C2 / C3 por separado',
      c4_n2: 'Pila Z Airyscan · 41 cortes · 16 bits · 37,8 µm medidos',
      a_c4a: 'TifMrcView mostrando un TIFF de fluorescencia de tres canales con las casillas C1, C2 y C3 en la barra de herramientas',
      a_c4b: 'TifMrcView mostrando una pila Z de Zeiss CZI Airyscan en fluorescencia verde con barra de escala y una medida de 37,8 µm',
      n_h: '<span>Descarga pequeña.</span> <span>Sin concesiones.</span>',
      n_win: 'Instalador de Windows', n_mac: 'macOS Universal, Apple Silicon e Intel',
      n_os: 'Plataformas, todas nativas', n_lang: 'Idiomas de interfaz, según tu sistema',
      fm_h: '<span>16 formatos.</span> <span>Un doble clic.</span>',
      fm_p: 'Incluidas pilas de ImageJ de más de 4 GB y 8 modos de datos MRC. Los formatos confocales (Zeiss, Leica, Nikon, Olympus), Imaris IMS, las imágenes de ME de Gatan DM3 / DM4, Thermo Fisher SER / EMD / EER y TVIPS y los parámetros de adquisición de SmartSEM llegan con los complementos.',
      fm_plug: 'complemento',
      mo_h: 'Y mucho más.',
      mo1t: 'Miniaturas de carpeta', mo1d: 'Las imágenes de la carpeta, en una barra lateral agrupadas por formato; los formatos que no son de ME aparecen plegados. Cambia con ↑ ↓.',
      mo2t: 'Contraste automático por corte', mo2d: 'Saturación del 0,1 % por defecto, o arrastra el rango sobre el histograma.',
      mo3t: 'Reproducción Z / T', mo3d: 'Espacio reproduce Z; las series temporales tienen también control T.',
      mo4t: 'Recortar y exportar', mo4d: 'Enmarca una región para verla sola y expórtala a PNG a resolución completa.',
      mo5t: 'Información de imagen', mo5d: 'Formato, tamaño, tamaño de píxel y parámetros de adquisición en un solo sitio, con búsqueda y copia.',
      mo6t: 'Actualizaciones automáticas', mo6d: 'Un aviso te indica que hay versión nueva. Un clic la descarga, verifica la firma y reinicia.',
      pl_eye: 'Plugins',
      pl_h2: '<span>Ampliable sin límites.</span>',
      pl_p: 'Instala solo lo que necesitas. Los complementos incluso pueden añadir formatos de archivo nuevos: se ejecutan en un entorno aislado WebAssembly, un mismo zip sirve para los tres sistemas y leen tan rápido como los formatos integrados.',
      pg1t: 'Grabador de vídeo', pg1d: 'Graba series Z o T en MP4 con tu contraste, canales, anotaciones y barra de escala.',
      pg2d: 'Todos los formatos de Zeiss en un complemento: CZI de ZEN, LSM de ZEN black y los parámetros de SmartSEM con barra de escala calibrada.',
      pg3d: 'Un complemento por marca: LIF / LOF, ND2, OIB / OIR e IMS, con el objetivo, los láseres, los detectores y más junto a la imagen.',
      pg4d: 'Formatos de microscopía electrónica de Gatan, Thermo Fisher y TVIPS: DM3 / DM4, SER, Velox EMD y EER, con voltaje, aumento y tamaño de píxel.',
      dl_h: '<span>Descarga TifMrcView.</span>',
      dl_p: 'Gratis. Instaladores nativos para macOS, Windows y Linux<span class="an">, y un APK para Android</span>.',
      k_tool: 'Seleccionar · mover · medir · lápiz · recuadro · círculo · recortar', k_undo: 'Deshacer / rehacer', k_annot: 'Mostrar / ocultar anotaciones',
      st_h: '<span>Código abierto.</span> <span>Gratis.</span>',
      st_p: 'Si te gusta, dale una ⭐ en GitHub. Anima mucho al autor y ayuda a que otros lo encuentren.',
      star_btn: 'Dar Star en GitHub', star_foot: '⭐ Star', sc_close: 'Cerrar'
    },
    fr: {
      title: 'TifMrcView · Visionneuse d’images de microscopie et de ME',
      desc: 'TifMrcView : une visionneuse légère, rapide et pratique pour la microscopie optique et électronique, gratuite et open source. Des piles TIFF / OME-TIFF et MRC de cryo-ME de plusieurs Go s’ouvrent d’un double-clic ; vues XY / XZ / YZ, mesures en unités physiques, fluorescence multicanal ; des extensions ajoutent de nouveaux formats comme Zeiss CZI. Pour macOS, Windows, Linux et Android.',
      h1: '<span>TIFF et MRC,</span> <span>enfin rapides.</span>',
      slogan: 'Léger. Rapide. Simple. Gratuit et open source.',
      lead: 'Une visionneuse pour la microscopie optique et électronique : des piles TIFF, OME‑TIFF et MRC de plusieurs gigaoctets s’ouvrent d’un double-clic. Avec des extensions, ouvrez aussi les fichiers confocaux de Zeiss, Leica, Nikon et Olympus (CZI, LSM, LIF, ND2, OIR) et Imaris IMS, les fichiers de microscopie électronique Gatan, Thermo Fisher et TVIPS (DM3 / DM4, SER, EMD, EER), et lisez leurs paramètres d’acquisition. Sur macOS, Windows et Linux<span class="an">, et aussi Android</span>.',
      an_a: 'Télécharger l’APK Android', pro_line: 'Segmentation et vue 3D : <b>TifMrcView Pro</b> →',
      dl_apk: 'Télécharger l’APK', note_apk: 'Android 7 ou plus, arm64',
      an_tip: 'La visionneuse de base pour téléphones et tablettes (sans les fonctions Pro). Autorisez « Installer des applis inconnues » si demandé. Pincez pour zoomer ; appui long sur une annotation pour la supprimer.',
      demo_try: 'Cliquez pour essayer : déplacez Z, mesurez, réglez luminosité et contraste', demo_full: 'Ouvrir en plein écran', demo_title: 'Démo en direct de TifMrcView',
      demo_note: 'La vraie interface, dans votre navigateur. Les exemples sont des images de fluorescence Zeiss Airyscan réduites.',
      a_hero: 'Fenêtre principale de TifMrcView avec un tomogramme de cryo-microscopie électronique (MRC) et une ligne mesurée de 864,2 nm, le panneau de contraste et d’annotation à gauche et les vignettes du dossier à droite',
      c1_eye: 'Cryo-ET · MRC',
      c1_h: '<span>Des tomogrammes de plusieurs Go.</span> <span>Sans attendre.</span>',
      c1_p: 'Les fichiers sont mappés en mémoire et décodés sur tous les cœurs ; les coupes les plus proches sont préparées en premier, en arrière-plan. Faites glisser le curseur Z : l’image suit.',
      c1_s1: 'Taille de chaque tomogramme', c1_s2: 'Coupes Z, en direct sous le curseur',
      c1_note: 'Données affichées : 1024 × 1024 × 512, float32, 15,244 Å/px.',
      a_c1: 'Un tomogramme de cryo-ME de 512 coupes à la coupe 277 ; la barre de vignettes liste des fichiers MRC de 2,0 Go du même dossier',
      c2_eye: 'XY · XZ · YZ',
      c2_h: '<span>Tournez le volume.</span> <span>Voyez la structure.</span>',
      c2_p: 'Passez en vue latérale d’un clic et tranchez toute la pile le long de Z. Tracez un cadre sur n’importe quel plan : largeur et hauteur s’affichent sur l’image.',
      c2_note: 'Vue latérale XZ · 1,35 µm × 0,252 µm',
      a_c2: 'Le même tomogramme en vue latérale XZ avec un cadre de 1,35 µm × 0,252 µm et une barre d’échelle de 200 nm',
      c3_eye: 'Mesurer et annoter',
      c3_h: '<span>Chaque trait,</span> <span>à l’échelle.</span>',
      c3_p: 'Lignes, cadres, cercles et crayon libre, convertis en nm et µm d’après la taille de pixel.',
      c3_l1: 'Une barre d’échelle conservée dans chaque PNG exporté',
      c3_l2: 'Les modèles IMOD (.mod / .fid) du même nom, superposés automatiquement',
      c3_l3: 'Des annotations par plan Z, enregistrées automatiquement',
      c3_l4: 'Tracer, déplacer, supprimer, recadrer : tout s’annule',
      a_c3: 'Le panneau d’annotation avec crayon libre, cadre et cercle, couleur et épaisseur, modèle IMOD et barre d’échelle, à côté d’une mesure de 864,2 nm',
      c4_eye: 'Fluorescence · Multicanal',
      c4_h: '<span>Chaque canal,</span> <span>juste comme il faut.</span>',
      c4_p: 'Affichez, recolorez et réglez le contraste de chaque canal séparément, avec une plage d’affichage automatique sur chaque coupe. L’OME-TIFF et les hyperstacks ImageJ sont lus nativement ; installez l’extension correspondante pour ouvrir les fichiers confocaux de Zeiss (CZI, LSM), Leica (LIF, LOF), Nikon (ND2) et Olympus (OIB, OIR), ainsi que Imaris IMS, avec l’objectif, le laser et le détecteur à côté de l’image.',
      c4_t1: 'TIFF multicanal', c4_t2: 'Zeiss CZI · Airyscan',
      c4_n1: '4072 × 4072 · 3 canaux · C1 / C2 / C3 séparément',
      c4_n2: 'Pile Z Airyscan · 41 coupes · 16 bits · 37,8 µm mesurés',
      a_c4a: 'TifMrcView affichant un TIFF de fluorescence à trois canaux avec les cases C1, C2 et C3 dans la barre d’outils',
      a_c4b: 'TifMrcView affichant une pile Z Zeiss CZI Airyscan en fluorescence verte avec barre d’échelle et une mesure de 37,8 µm',
      n_h: '<span>Petit téléchargement.</span> <span>Aucun compromis.</span>',
      n_win: 'Installateur Windows', n_mac: 'macOS Universel, puces Apple et Intel',
      n_os: 'Plateformes, toutes natives', n_lang: 'Langues d’interface, selon votre système',
      fm_h: '<span>16 formats.</span> <span>Un double-clic.</span>',
      fm_p: 'Y compris les piles ImageJ de plus de 4 Go et 8 modes de données MRC. Les formats confocaux (Zeiss, Leica, Nikon, Olympus), Imaris IMS, les images MET Gatan DM3 / DM4, Thermo Fisher SER / EMD / EER et TVIPS et les paramètres d’acquisition SmartSEM sont fournis par des extensions.',
      fm_plug: 'extension',
      mo_h: 'Et bien plus encore.',
      mo1t: 'Vignettes du dossier', mo1d: 'Les images du dossier dans une barre latérale, groupées par format ; les formats hors ME sont repliés. ↑ ↓ pour changer.',
      mo2t: 'Contraste auto par coupe', mo2d: '0,1 % de saturation par défaut, ou faites glisser la plage sur l’histogramme.',
      mo3t: 'Lecture Z / T', mo3d: 'Espace lit les plans Z ; les séries temporelles ont aussi un curseur T.',
      mo4t: 'Recadrer et exporter', mo4d: 'Encadrez une zone pour ne voir qu’elle, puis exportez-la en PNG en pleine résolution.',
      mo5t: 'Infos image', mo5d: 'Format, taille, taille de pixel et paramètres d’acquisition au même endroit, consultables et copiables.',
      mo6t: 'Mises à jour automatiques', mo6d: 'Un bandeau signale la nouvelle version. Un clic la télécharge, vérifie la signature et redémarre.',
      pl_eye: 'Plugins',
      pl_h2: '<span>Extensible à l’infini.</span>',
      pl_p: 'N’installez que ce dont vous avez besoin. Les extensions peuvent même ajouter de nouveaux formats : elles tournent dans un bac à sable WebAssembly, un seul zip fonctionne sur les trois systèmes et la lecture est aussi rapide qu’avec les formats intégrés.',
      pg1t: 'Enregistreur vidéo', pg1d: 'Enregistre les séries Z ou T en MP4, avec contraste, canaux, annotations et barre d’échelle.',
      pg2d: 'Tous les formats Zeiss dans une extension : CZI de ZEN, LSM de ZEN black et paramètres SmartSEM avec barre d’échelle calibrée.',
      pg3d: 'Une extension par marque : LIF / LOF, ND2, OIB / OIR et IMS, avec l’objectif, les lasers, les détecteurs et plus à côté de l’image.',
      pg4d: 'Formats de microscopie électronique Gatan, Thermo Fisher et TVIPS : DM3 / DM4, SER, Velox EMD et EER, avec tension, grandissement et taille de pixel.',
      dl_h: '<span>Téléchargez TifMrcView.</span>',
      dl_p: 'Gratuit. Installateurs natifs pour macOS, Windows et Linux<span class="an">, plus un APK Android</span>.',
      k_tool: 'Sélection · déplacer · mesurer · crayon · cadre · cercle · recadrer', k_undo: 'Annuler / rétablir', k_annot: 'Afficher / masquer les annotations',
      st_h: '<span>Open source.</span> <span>Gratuit.</span>',
      st_p: 'Vous l’aimez ? Offrez-lui une ⭐ sur GitHub. Cela encourage l’auteur et aide d’autres personnes à le découvrir.',
      star_btn: 'Star sur GitHub', star_foot: '⭐ Star', sc_close: 'Fermer'
    },
    ru: {
      title: 'TifMrcView · Просмотрщик изображений микроскопии и ЭМ',
      desc: 'TifMrcView: маленький, быстрый и удобный просмотрщик для световой и электронной микроскопии, бесплатный и с открытым кодом. Стеки TIFF / OME-TIFF и MRC для крио-ЭМ в несколько ГБ открываются двойным щелчком; виды XY / XZ / YZ, измерения в физических единицах, многоканальная флуоресценция; плагины добавляют новые форматы, например Zeiss CZI. Для macOS, Windows, Linux и Android.',
      h1: '<span>TIFF и MRC —</span> <span>так быстро, как нужно.</span>',
      slogan: 'Лёгкий. Быстрый. Удобный. Бесплатный, с открытым кодом.',
      lead: 'Просмотрщик изображений световой и электронной микроскопии: стеки TIFF, OME‑TIFF и MRC в несколько гигабайт открываются двойным щелчком. С плагинами открываются конфокальные файлы Zeiss, Leica, Nikon и Olympus (CZI, LSM, LIF, ND2, OIR) и Imaris IMS, файлы электронной микроскопии Gatan, Thermo Fisher и TVIPS (DM3 / DM4, SER, EMD, EER), а также читаются параметры съёмки. На macOS, Windows и Linux<span class="an">, а также Android</span>.',
      an_a: 'Скачать APK для Android', pro_line: 'Сегментация и 3D: <b>TifMrcView Pro</b> →',
      dl_apk: 'Скачать APK', note_apk: 'Android 7 и новее, arm64',
      an_tip: 'Базовый просмотрщик для телефонов и планшетов (без функций Pro). Разрешите «Установку неизвестных приложений», когда попросят. Масштаб — двумя пальцами; долгое нажатие на пометку удаляет её.',
      demo_try: 'Нажмите, чтобы попробовать: Z, измерения, яркость и контраст', demo_full: 'Открыть на весь экран', demo_title: 'Живая демонстрация TifMrcView',
      demo_note: 'Настоящий интерфейс прямо в браузере. Примеры — уменьшенные флуоресцентные снимки Zeiss Airyscan.',
      a_hero: 'Главное окно TifMrcView: криоэлектронная томограмма (MRC) с измеренной линией 864,2 нм, слева панель контраста и аннотаций, справа миниатюры папки',
      c1_eye: 'Крио-ЭТ · MRC',
      c1_h: '<span>Гигабайтные томограммы.</span> <span>Без ожидания.</span>',
      c1_p: 'Файлы отображаются в память и декодируются на всех ядрах, а ближайшие срезы готовятся в фоне первыми. Тяните ползунок Z — изображение успевает за рукой.',
      c1_s1: 'Размер каждой томограммы', c1_s2: 'Срезов Z — вживую при перетаскивании',
      c1_note: 'Показанные данные: 1024 × 1024 × 512, float32, 15,244 Å/пикс.',
      a_c1: 'Криоэлектронная томограмма из 512 срезов на срезе 277; в ленте миниатюр — MRC-файлы по 2,0 ГБ из той же папки',
      c2_eye: 'XY · XZ · YZ',
      c2_h: '<span>Поверните объём.</span> <span>Увидьте структуру.</span>',
      c2_p: 'Один щелчок — и вы в боковом виде, разрезающем весь стек вдоль Z. Нарисуйте рамку в любой плоскости: ширина и высота появятся прямо на изображении.',
      c2_note: 'Боковой вид XZ · 1,35 мкм × 0,252 мкм',
      a_c2: 'Та же томограмма в боковом виде XZ с рамкой 1,35 мкм × 0,252 мкм и масштабной линейкой 200 нм',
      c3_eye: 'Измерения и аннотации',
      c3_h: '<span>Каждый штрих —</span> <span>в масштабе.</span>',
      c3_p: 'Линии, рамки, круги и свободное перо — в нм и мкм по размеру пикселя.',
      c3_l1: 'Масштабная линейка, которая остаётся в каждом экспортированном PNG',
      c3_l2: 'Одноимённые модели IMOD (.mod / .fid) накладываются автоматически',
      c3_l3: 'Аннотации хранятся по срезам Z и сохраняются автоматически',
      c3_l4: 'Рисование, перемещение, удаление и обрезку можно отменить',
      a_c3: 'Панель аннотаций: свободное перо, рамка и круг, цвет и толщина, модель IMOD и масштабная линейка; рядом измерение 864,2 нм',
      c4_eye: 'Флуоресценция · Многоканальность',
      c4_h: '<span>Каждый канал —</span> <span>как надо.</span>',
      c4_p: 'Включайте, перекрашивайте и настраивайте контраст каждого канала отдельно; диапазон отображения подбирается на каждом срезе автоматически. OME-TIFF и гиперстеки ImageJ читаются напрямую, а с нужным плагином открываются конфокальные файлы Zeiss (CZI, LSM), Leica (LIF, LOF), Nikon (ND2) и Olympus (OIB, OIR), а также Imaris IMS — объектив, лазер и детектор показываются рядом с изображением.',
      c4_t1: 'Многоканальный TIFF', c4_t2: 'Zeiss CZI · Airyscan',
      c4_n1: '4072 × 4072 · 3 канала · C1 / C2 / C3 по отдельности',
      c4_n2: 'Z-стек Airyscan · 41 срез · 16 бит · измерено 37,8 мкм',
      a_c4a: 'TifMrcView показывает трёхканальный флуоресцентный TIFF с флажками C1, C2 и C3 на панели инструментов',
      a_c4b: 'TifMrcView показывает Z-стек Zeiss CZI Airyscan в зелёной флуоресценции с масштабной линейкой и измерением 37,8 мкм',
      n_h: '<span>Маленькая загрузка.</span> <span>Без компромиссов.</span>',
      n_win: 'Установщик для Windows', n_mac: 'macOS Universal, Apple Silicon и Intel',
      n_os: 'Платформы, все нативные', n_lang: 'Языков интерфейса, по языку системы',
      fm_h: '<span>16 форматов.</span> <span>Один двойной щелчок.</span>',
      fm_p: 'Включая стеки ImageJ больше 4 ГБ и 8 режимов данных MRC. Конфокальные форматы (Zeiss, Leica, Nikon, Olympus), Imaris IMS, ЭМ-изображения Gatan DM3 / DM4, Thermo Fisher SER / EMD / EER и TVIPS и параметры съёмки SmartSEM дают плагины.',
      fm_plug: 'плагин',
      mo_h: 'И многое другое.',
      mo1t: 'Миниатюры папки', mo1d: 'Изображения из той же папки — в боковой ленте, по форматам; не-ЭМ форматы свёрнуты. Переключение ↑ ↓.',
      mo2t: 'Автоконтраст по срезам', mo2d: 'По умолчанию 0,1 % насыщения — или выделите диапазон прямо на гистограмме.',
      mo3t: 'Воспроизведение Z / T', mo3d: 'Пробел проигрывает Z; для временных рядов есть ещё ползунок T.',
      mo4t: 'Обрезка и экспорт', mo4d: 'Выделите область, чтобы смотреть только её, и экспортируйте PNG в исходном разрешении.',
      mo5t: 'Сведения об изображении', mo5d: 'Формат, размер, размер пикселя и параметры съёмки в одном месте — с поиском и копированием.',
      mo6t: 'Автообновление', mo6d: 'Баннер сообщит о новой версии. Один щелчок — загрузка, проверка подписи и перезапуск.',
      pl_eye: 'Плагины',
      pl_h2: '<span>Расширяйте без границ.</span>',
      pl_p: 'Ставьте только то, что нужно. Плагины могут добавлять даже новые форматы файлов: они работают в песочнице WebAssembly, один zip подходит для всех трёх систем, а чтение не медленнее встроенных форматов.',
      pg1t: 'Запись видео', pg1d: 'Записывает серии Z или T в MP4 с текущим контрастом, каналами, аннотациями и линейкой.',
      pg2d: 'Все форматы Zeiss в одном плагине: CZI из ZEN, LSM из ZEN black и параметры SmartSEM с калиброванной линейкой.',
      pg3d: 'По одному плагину на бренд: LIF / LOF, ND2, OIB / OIR и IMS — объектив, лазеры, детекторы и другое рядом с изображением.',
      pg4d: 'Форматы электронной микроскопии Gatan, Thermo Fisher и TVIPS: DM3 / DM4, SER, Velox EMD и EER — с напряжением, увеличением и размером пикселя.',
      dl_h: '<span>Скачать TifMrcView.</span>',
      dl_p: 'Бесплатно. Нативные установщики для macOS, Windows и Linux<span class="an">, а также APK для Android</span>.',
      k_tool: 'Выбор · перемещение · измерение · перо · рамка · круг · обрезка', k_undo: 'Отменить / повторить', k_annot: 'Показать / скрыть аннотации',
      st_h: '<span>Открытый код.</span> <span>Бесплатно.</span>',
      st_p: 'Если нравится — поставьте ⭐ на GitHub. Это лучшая поддержка автору, и так проект найдут другие.',
      star_btn: 'Star на GitHub', star_foot: '⭐ Star', sc_close: 'Закрыть'
    }
  };
  Object.keys(HOME).forEach(function (c) { for (var k in HOME[c]) T[c][k] = HOME[c][k]; });
  // TifMrcView Pro page (site/pro/) and the links to it.
  var PRO = {
    zh: {
      nav_pro: "Pro 版",
      pro_teaser: "要做 3D、分割和电镜分析？看看 <b>TifMrcView Pro</b> →",
      pro_title: "TifMrcView Pro · 3D、分割与电镜分析",
      pro_desc: "TifMrcView Pro：免费开源的显微与电镜图像分析工具。3D 体视图、分割标签与材料、SAM 2.1 点击分割并沿 Z 传播、CTF 拟合、漂移校正，计算尽量在 GPU 上。",
      pro_h1: "<span>不只是看，</span><span>还能分析。</span>",
      pro_lead: "TifMrcView Pro 是在查看器之上做成的分析工具：3D 体视图、分割、SAM 2.1 智能分割、CTF 拟合和漂移校正。同样免费开源，计算尽量放在 GPU 上（CUDA、Metal、DX12、Vulkan）。",
      pro_cta: "下载 Pro",
      pro_f1h: "3D 体视图",
      pro_f1p: "最大强度投影、半透明和等值面渲染。像 ChimeraX 那样按 σ 调阈值，平滑和直方图都在 GPU 上算。冷冻电镜断层可一键反相。",
      pro_f2h: "分割标签与材料",
      pro_f2p: "画笔、魔棒、阈值、分水岭、套索和层间插值。材料像 Amira 那样可命名、配色、隐藏和锁定，存成 Fiji / napari 能读的 16 位 TIFF。",
      pro_f3h: "SAM 2.1 智能分割",
      pro_f3p: "点一下就分出物体，再沿 Z 自动传播到整个物体，细膜和闭合轮廓也能跟住。模型用 Rust 重写，在你的显卡上本地运行，数据不出电脑。",
      pro_f4h: "CTF 拟合",
      pro_f4p: "在功率谱上拟合离焦、像散和相位板相移，把拟合出的 CTF 零点画在谱上，并给出一维拟合曲线。",
      pro_f5h: "漂移校正",
      pro_f5p: "测出影片、倾转系列和连续切片每一层的位移，画出轨迹，保存对齐后的栈或叠加图（MRC）。",
      pro_cmp_h: "两个版本，各有侧重",
      pro_b_h: "基础版：轻巧的查看器",
      pro_b_p: "小而快，双击就开数 GB 的 TIFF / MRC。手动测量、标注、FFT 和能谱查看都有。适合日常浏览和快速检查，也有安卓版。",
      pro_p_h: "Pro：分析工具",
      pro_p_p: "包含基础版的全部功能，另加 3D、分割、SAM、CTF 拟合和漂移校正。适合要出结果的分析工作。可以和基础版同时安装。",
      pro_b_a: "基础版介绍",
      pro_dl_h: "<span>下载 TifMrcView Pro。</span>",
      pro_dl_p: "免费开源。macOS、Windows、Linux 原生安装包。",
      pro_note: "第一次使用 SAM 时会下载约 156 MB 的模型。Linux 版自带 CUDA 支持（NVIDIA 显卡）。",
      pro_mac_tip: "首次打开若被拦截：在终端运行 <code>xattr -dr com.apple.quarantine \"/Applications/TifMrcView Pro.app\"</code>",
      pro_soon: "Pro 安装包即将发布。"
    },
    en: {
      nav_pro: "Pro",
      pro_teaser: "Need 3D, segmentation and EM analysis? See <b>TifMrcView Pro</b> →",
      pro_title: "TifMrcView Pro · 3D, segmentation and EM analysis",
      pro_desc: "TifMrcView Pro: a free, open-source analysis tool for microscopy and EM images. 3D volume view, segmentation labels and materials, SAM 2.1 click segmentation along Z, CTF fitting and drift correction, computed on the GPU wherever possible.",
      pro_h1: "<span>Not just viewing.</span> <span>Analysis.</span>",
      pro_lead: "TifMrcView Pro builds an analysis tool on top of the viewer: 3D volume view, segmentation, SAM 2.1 smart segmentation, CTF fitting and drift correction. Also free and open source, computed on the GPU wherever possible (CUDA, Metal, DX12, Vulkan).",
      pro_cta: "Download Pro",
      pro_f1h: "3D volume view",
      pro_f1p: "Maximum intensity, translucent and surface rendering. Levels in σ like ChimeraX, with smoothing and histograms computed on the GPU. Cryo-ET tomograms invert in one click.",
      pro_f2h: "Segmentation labels and materials",
      pro_f2p: "Brush, magic wand, threshold, watershed, lasso and slice interpolation. Materials as in Amira: named, coloured, hidden and locked, saved as a 16-bit TIFF that Fiji and napari read.",
      pro_f3h: "SAM 2.1 smart segmentation",
      pro_f3p: "One click segments an object, then it is carried along Z through the whole object, thin membranes and closed outlines included. The model is rewritten in Rust and runs locally on your GPU; your data never leaves your computer.",
      pro_f4h: "CTF fitting",
      pro_f4p: "Fits defocus, astigmatism and phase-plate shift to the power spectrum, draws the fitted CTF zeros over it and shows the 1D fit.",
      pro_f5h: "Drift correction",
      pro_f5p: "Measures how far each frame of a movie, tilt series or serial section stack moved, plots the trajectory, and saves the aligned stack or its sum as MRC.",
      pro_cmp_h: "Two editions, two jobs",
      pro_b_h: "Standard: a light viewer",
      pro_b_p: "Small and fast; multi-GB TIFF / MRC files open with a double-click. Manual measurement, annotation, FFT and spectrum viewing included. Made for everyday browsing and quick checks, also on Android.",
      pro_p_h: "Pro: an analysis tool",
      pro_p_p: "Everything in the standard app, plus 3D, segmentation, SAM, CTF fitting and drift correction. Made for analysis that has to produce results. Installs side by side with the standard app.",
      pro_b_a: "About the standard app",
      pro_dl_h: "<span>Download TifMrcView Pro.</span>",
      pro_dl_p: "Free and open source. Native installers for macOS, Windows and Linux.",
      pro_note: "The first use of SAM downloads a model of about 156 MB. The Linux build includes CUDA support (NVIDIA GPUs).",
      pro_mac_tip: "If macOS blocks the first launch, run <code>xattr -dr com.apple.quarantine \"/Applications/TifMrcView Pro.app\"</code> in Terminal.",
      pro_soon: "Pro installers are coming shortly."
    },
    de: {
      nav_pro: "Pro",
      pro_teaser: "3D, Segmentierung und EM-Analyse gefragt? Zu <b>TifMrcView Pro</b> →",
      pro_title: "TifMrcView Pro · 3D, Segmentierung und EM-Analyse",
      pro_desc: "TifMrcView Pro: kostenloses Open-Source-Analysewerkzeug für Mikroskopie- und EM-Bilder. 3D-Volumenansicht, Segmentierungs-Labels und Materialien, SAM-2.1-Segmentierung per Klick entlang Z, CTF-Fit und Driftkorrektur, wo möglich auf der GPU.",
      pro_h1: "<span>Nicht nur ansehen.</span> <span>Analysieren.</span>",
      pro_lead: "TifMrcView Pro macht aus dem Viewer ein Analysewerkzeug: 3D-Volumenansicht, Segmentierung, intelligente Segmentierung mit SAM 2.1, CTF-Fit und Driftkorrektur. Ebenfalls kostenlos und Open Source, gerechnet wo möglich auf der GPU (CUDA, Metal, DX12, Vulkan).",
      pro_cta: "Pro herunterladen",
      pro_f1h: "3D-Volumenansicht",
      pro_f1p: "Maximumprojektion, transluzente und Oberflächendarstellung. Schwellen in σ wie in ChimeraX, Glättung und Histogramme auf der GPU. Kryo-ET-Tomogramme lassen sich mit einem Klick invertieren.",
      pro_f2h: "Segmentierungs-Labels und Materialien",
      pro_f2p: "Pinsel, Zauberstab, Schwelle, Watershed, Lasso und Interpolation zwischen Schichten. Materialien wie in Amira: benennen, einfärben, ausblenden, sperren; gespeichert als 16-Bit-TIFF, das Fiji und napari lesen.",
      pro_f3h: "Intelligente Segmentierung mit SAM 2.1",
      pro_f3p: "Ein Klick segmentiert ein Objekt, das dann entlang Z durch das ganze Objekt verfolgt wird – auch dünne Membranen und geschlossene Umrisse. Das Modell ist in Rust neu geschrieben und läuft lokal auf Ihrer GPU; die Daten verlassen den Rechner nicht.",
      pro_f4h: "CTF-Fit",
      pro_f4p: "Passt Defokus, Astigmatismus und Phasenplatten-Verschiebung an das Leistungsspektrum an, zeichnet die CTF-Nullstellen ein und zeigt den 1D-Fit.",
      pro_f5h: "Driftkorrektur",
      pro_f5p: "Misst, wie weit sich jedes Bild eines Films, einer Kippserie oder eines Serienschnitt-Stapels verschoben hat, zeigt die Trajektorie und speichert den ausgerichteten Stapel oder seine Summe als MRC.",
      pro_cmp_h: "Zwei Editionen, zwei Aufgaben",
      pro_b_h: "Standard: ein leichter Viewer",
      pro_b_p: "Klein und schnell; mehrere GB große TIFF / MRC öffnen per Doppelklick. Manuelles Messen, Annotationen, FFT und Spektrenansicht inklusive. Für den Alltag und schnelle Kontrollen, auch für Android.",
      pro_p_h: "Pro: ein Analysewerkzeug",
      pro_p_p: "Alles aus der Standard-App, dazu 3D, Segmentierung, SAM, CTF-Fit und Driftkorrektur. Für Analysen, die Ergebnisse liefern sollen. Lässt sich neben der Standard-App installieren.",
      pro_b_a: "Zur Standard-App",
      pro_dl_h: "<span>TifMrcView Pro herunterladen.</span>",
      pro_dl_p: "Kostenlos und Open Source. Native Installer für macOS, Windows und Linux.",
      pro_note: "Beim ersten Einsatz von SAM wird ein Modell von etwa 156 MB geladen. Die Linux-Version bringt CUDA-Unterstützung mit (NVIDIA-GPUs).",
      pro_mac_tip: "Blockiert macOS den ersten Start, im Terminal <code>xattr -dr com.apple.quarantine \"/Applications/TifMrcView Pro.app\"</code> ausführen.",
      pro_soon: "Die Pro-Installer erscheinen in Kürze."
    },
    ja: {
      nav_pro: "Pro 版",
      pro_teaser: "3D・セグメンテーション・電顕解析が必要なら <b>TifMrcView Pro</b> へ →",
      pro_title: "TifMrcView Pro · 3D・セグメンテーション・電顕解析",
      pro_desc: "TifMrcView Pro：顕微鏡・電子顕微鏡画像のための無料オープンソース解析ツール。3D ボリューム表示、セグメンテーションラベルとマテリアル、Z 方向に伝播する SAM 2.1 クリックセグメンテーション、CTF フィッティング、ドリフト補正。計算はできる限り GPU で。",
      pro_h1: "<span>見るだけでなく、</span><span>解析まで。</span>",
      pro_lead: "TifMrcView Pro はビューアの上に作った解析ツールです。3D ボリューム表示、セグメンテーション、SAM 2.1 によるスマートセグメンテーション、CTF フィッティング、ドリフト補正。こちらも無料・オープンソースで、計算はできる限り GPU（CUDA・Metal・DX12・Vulkan）で行います。",
      pro_cta: "Pro をダウンロード",
      pro_f1h: "3D ボリューム表示",
      pro_f1p: "最大値投影・半透明・等値面レンダリング。ChimeraX のように σ でしきい値を調整し、平滑化とヒストグラムは GPU で計算。クライオ ET トモグラムはワンクリックで反転。",
      pro_f2h: "セグメンテーションラベルとマテリアル",
      pro_f2p: "ブラシ、マジックワンド、しきい値、ウォーターシェッド、投げ縄、スライス間補間。Amira のようにマテリアルの名前・色・表示・ロックを設定でき、Fiji や napari で読める 16 ビット TIFF に保存。",
      pro_f3h: "SAM 2.1 スマートセグメンテーション",
      pro_f3p: "ワンクリックで物体を切り出し、Z 方向に物体全体へ自動伝播。薄い膜や閉じた輪郭にも追従します。モデルは Rust で書き直され、お使いの GPU 上でローカルに動作。データは PC の外に出ません。",
      pro_f4h: "CTF フィッティング",
      pro_f4p: "パワースペクトルにデフォーカス・非点収差・位相板シフトをフィットし、CTF のゼロ点をスペクトル上に描画、1 次元フィットも表示します。",
      pro_f5h: "ドリフト補正",
      pro_f5p: "ムービー、傾斜シリーズ、連続切片の各フレームのずれを測ってトラジェクトリを描き、位置合わせしたスタックまたは積算像を MRC で保存します。",
      pro_cmp_h: "ふたつの版、それぞれの役割",
      pro_b_h: "標準版：軽快なビューア",
      pro_b_p: "小さく速く、数 GB の TIFF / MRC もダブルクリックで開きます。手動計測、注釈、FFT、スペクトル表示つき。日常の閲覧や手早い確認に。Android 版もあります。",
      pro_p_h: "Pro：解析ツール",
      pro_p_p: "標準版の全機能に加え、3D、セグメンテーション、SAM、CTF フィッティング、ドリフト補正。結果を出す解析作業に。標準版と同時にインストールできます。",
      pro_b_a: "標準版の紹介",
      pro_dl_h: "<span>TifMrcView Pro をダウンロード。</span>",
      pro_dl_p: "無料・オープンソース。macOS・Windows・Linux のネイティブインストーラ。",
      pro_note: "SAM を初めて使うときに約 156 MB のモデルをダウンロードします。Linux 版は CUDA（NVIDIA GPU）に対応しています。",
      pro_mac_tip: "初回起動がブロックされたら、ターミナルで <code>xattr -dr com.apple.quarantine \"/Applications/TifMrcView Pro.app\"</code> を実行してください。",
      pro_soon: "Pro のインストーラはまもなく公開します。"
    },
    es: {
      nav_pro: "Pro",
      pro_teaser: "¿Necesitas 3D, segmentación y análisis de ME? Mira <b>TifMrcView Pro</b> →",
      pro_title: "TifMrcView Pro · 3D, segmentación y análisis de ME",
      pro_desc: "TifMrcView Pro: herramienta de análisis gratuita y de código abierto para imágenes de microscopía y ME. Vista 3D de volumen, etiquetas y materiales de segmentación, segmentación SAM 2.1 con un clic a lo largo de Z, ajuste de CTF y corrección de deriva, en la GPU siempre que se puede.",
      pro_h1: "<span>No solo ver.</span> <span>Analizar.</span>",
      pro_lead: "TifMrcView Pro convierte el visor en una herramienta de análisis: vista 3D de volumen, segmentación, segmentación inteligente con SAM 2.1, ajuste de CTF y corrección de deriva. También gratis y de código abierto, con los cálculos en la GPU siempre que se puede (CUDA, Metal, DX12, Vulkan).",
      pro_cta: "Descargar Pro",
      pro_f1h: "Vista 3D de volumen",
      pro_f1p: "Proyección de máxima intensidad, renderizado translúcido y de superficie. Umbrales en σ como en ChimeraX, con suavizado e histogramas en la GPU. Los tomogramas de crio-ET se invierten con un clic.",
      pro_f2h: "Etiquetas y materiales de segmentación",
      pro_f2p: "Pincel, varita mágica, umbral, watershed, lazo e interpolación entre cortes. Materiales como en Amira: con nombre, color, ocultables y bloqueables, guardados como TIFF de 16 bits que leen Fiji y napari.",
      pro_f3h: "Segmentación inteligente con SAM 2.1",
      pro_f3p: "Un clic segmenta un objeto y luego se propaga en Z por todo el objeto, incluidas membranas finas y contornos cerrados. El modelo está reescrito en Rust y corre localmente en tu GPU; tus datos no salen del ordenador.",
      pro_f4h: "Ajuste de CTF",
      pro_f4p: "Ajusta desenfoque, astigmatismo y desfase de placa de fase al espectro de potencia, dibuja los ceros de la CTF sobre él y muestra el ajuste 1D.",
      pro_f5h: "Corrección de deriva",
      pro_f5p: "Mide cuánto se movió cada fotograma de una película, serie de inclinación o cortes seriados, dibuja la trayectoria y guarda la pila alineada o su suma en MRC.",
      pro_cmp_h: "Dos ediciones, dos tareas",
      pro_b_h: "Estándar: un visor ligero",
      pro_b_p: "Pequeño y rápido; archivos TIFF / MRC de varios GB se abren con doble clic. Incluye medición manual, anotaciones, FFT y visor de espectros. Para el día a día y comprobaciones rápidas, también en Android.",
      pro_p_h: "Pro: una herramienta de análisis",
      pro_p_p: "Todo lo de la versión estándar, más 3D, segmentación, SAM, ajuste de CTF y corrección de deriva. Para análisis que deben dar resultados. Se instala junto a la versión estándar.",
      pro_b_a: "Sobre la versión estándar",
      pro_dl_h: "<span>Descargar TifMrcView Pro.</span>",
      pro_dl_p: "Gratis y de código abierto. Instaladores nativos para macOS, Windows y Linux.",
      pro_note: "El primer uso de SAM descarga un modelo de unos 156 MB. La versión para Linux incluye soporte CUDA (GPU NVIDIA).",
      pro_mac_tip: "Si macOS bloquea la primera apertura, ejecuta <code>xattr -dr com.apple.quarantine \"/Applications/TifMrcView Pro.app\"</code> en Terminal.",
      pro_soon: "Los instaladores de Pro llegan en breve."
    },
    fr: {
      nav_pro: "Pro",
      pro_teaser: "Besoin de 3D, de segmentation et d’analyse MET ? Voir <b>TifMrcView Pro</b> →",
      pro_title: "TifMrcView Pro · 3D, segmentation et analyse MET",
      pro_desc: "TifMrcView Pro : outil d’analyse gratuit et open source pour les images de microscopie et de MET. Vue 3D du volume, étiquettes et matériaux de segmentation, segmentation SAM 2.1 en un clic le long de Z, ajustement de la CTF et correction de dérive, sur le GPU autant que possible.",
      pro_h1: "<span>Pas seulement voir.</span> <span>Analyser.</span>",
      pro_lead: "TifMrcView Pro fait du visualiseur un outil d’analyse : vue 3D du volume, segmentation, segmentation intelligente avec SAM 2.1, ajustement de la CTF et correction de dérive. Gratuit et open source lui aussi, avec les calculs sur le GPU autant que possible (CUDA, Metal, DX12, Vulkan).",
      pro_cta: "Télécharger Pro",
      pro_f1h: "Vue 3D du volume",
      pro_f1p: "Projection d’intensité maximale, rendu translucide et de surface. Seuils en σ comme dans ChimeraX, lissage et histogrammes calculés sur le GPU. Les tomogrammes de cryo-ET s’inversent en un clic.",
      pro_f2h: "Étiquettes et matériaux de segmentation",
      pro_f2p: "Pinceau, baguette magique, seuil, ligne de partage des eaux, lasso et interpolation entre coupes. Matériaux comme dans Amira : nommés, colorés, masqués et verrouillés, enregistrés en TIFF 16 bits lisible par Fiji et napari.",
      pro_f3h: "Segmentation intelligente avec SAM 2.1",
      pro_f3p: "Un clic segmente un objet, puis il est suivi en Z sur tout l’objet, membranes fines et contours fermés compris. Le modèle est réécrit en Rust et tourne localement sur votre GPU ; vos données ne quittent pas l’ordinateur.",
      pro_f4h: "Ajustement de la CTF",
      pro_f4p: "Ajuste la défocalisation, l’astigmatisme et le déphasage de lame de phase au spectre de puissance, trace les zéros de la CTF ajustée et affiche l’ajustement 1D.",
      pro_f5h: "Correction de dérive",
      pro_f5p: "Mesure le déplacement de chaque image d’un film, d’une série d’inclinaison ou de coupes sériées, trace la trajectoire et enregistre la pile alignée ou sa somme en MRC.",
      pro_cmp_h: "Deux éditions, deux usages",
      pro_b_h: "Standard : un visualiseur léger",
      pro_b_p: "Petit et rapide ; des TIFF / MRC de plusieurs Go s’ouvrent d’un double-clic. Mesure manuelle, annotations, FFT et affichage des spectres inclus. Pour le quotidien et les vérifications rapides, aussi sur Android.",
      pro_p_h: "Pro : un outil d’analyse",
      pro_p_p: "Tout ce que fait la version standard, plus 3D, segmentation, SAM, ajustement de la CTF et correction de dérive. Pour les analyses qui doivent aboutir à des résultats. S’installe à côté de la version standard.",
      pro_b_a: "Découvrir la version standard",
      pro_dl_h: "<span>Télécharger TifMrcView Pro.</span>",
      pro_dl_p: "Gratuit et open source. Installateurs natifs pour macOS, Windows et Linux.",
      pro_note: "La première utilisation de SAM télécharge un modèle d’environ 156 Mo. La version Linux prend en charge CUDA (GPU NVIDIA).",
      pro_mac_tip: "Si macOS bloque le premier lancement, exécutez <code>xattr -dr com.apple.quarantine \"/Applications/TifMrcView Pro.app\"</code> dans le Terminal.",
      pro_soon: "Les installateurs Pro arrivent bientôt."
    },
    ru: {
      nav_pro: "Pro",
      pro_teaser: "Нужны 3D, сегментация и анализ ЭМ? Смотрите <b>TifMrcView Pro</b> →",
      pro_title: "TifMrcView Pro · 3D, сегментация и анализ ЭМ",
      pro_desc: "TifMrcView Pro: бесплатный инструмент анализа с открытым кодом для изображений световой и электронной микроскопии. 3D-просмотр объёма, метки и материалы сегментации, сегментация SAM 2.1 по щелчку вдоль Z, подгонка CTF и коррекция дрейфа, по возможности на GPU.",
      pro_h1: "<span>Не только смотреть.</span> <span>Анализировать.</span>",
      pro_lead: "TifMrcView Pro превращает просмотрщик в инструмент анализа: 3D-просмотр объёма, сегментация, умная сегментация SAM 2.1, подгонка CTF и коррекция дрейфа. Тоже бесплатно и с открытым кодом, вычисления по возможности на GPU (CUDA, Metal, DX12, Vulkan).",
      pro_cta: "Скачать Pro",
      pro_f1h: "3D-просмотр объёма",
      pro_f1p: "Проекция максимальной интенсивности, полупрозрачный и поверхностный рендеринг. Пороги в σ, как в ChimeraX, сглаживание и гистограммы считаются на GPU. Томограммы крио-ЭТ инвертируются одним щелчком.",
      pro_f2h: "Метки и материалы сегментации",
      pro_f2p: "Кисть, волшебная палочка, порог, водораздел, лассо и интерполяция между срезами. Материалы, как в Amira: имя, цвет, скрытие и блокировка; сохраняются в 16-битный TIFF, который читают Fiji и napari.",
      pro_f3h: "Умная сегментация SAM 2.1",
      pro_f3p: "Один щелчок выделяет объект, затем он прослеживается по Z через весь объект, включая тонкие мембраны и замкнутые контуры. Модель переписана на Rust и работает локально на вашей видеокарте; данные не покидают компьютер.",
      pro_f4h: "Подгонка CTF",
      pro_f4p: "Подгоняет дефокус, астигматизм и сдвиг фазовой пластинки по спектру мощности, рисует нули CTF поверх спектра и показывает одномерную подгонку.",
      pro_f5h: "Коррекция дрейфа",
      pro_f5p: "Измеряет сдвиг каждого кадра фильма, серии наклонов или серийных срезов, строит траекторию и сохраняет выровненный стек или его сумму в MRC.",
      pro_cmp_h: "Две версии, две задачи",
      pro_b_h: "Стандартная: лёгкий просмотрщик",
      pro_b_p: "Маленький и быстрый: TIFF / MRC размером в несколько гигабайт открываются двойным щелчком. Ручные измерения, аннотации, FFT и просмотр спектров включены. Для повседневного просмотра и быстрых проверок, есть версия для Android.",
      pro_p_h: "Pro: инструмент анализа",
      pro_p_p: "Всё из стандартной версии плюс 3D, сегментация, SAM, подгонка CTF и коррекция дрейфа. Для анализа, который должен дать результат. Устанавливается рядом со стандартной версией.",
      pro_b_a: "О стандартной версии",
      pro_dl_h: "<span>Скачать TifMrcView Pro.</span>",
      pro_dl_p: "Бесплатно и с открытым кодом. Нативные установщики для macOS, Windows и Linux.",
      pro_note: "При первом запуске SAM загружается модель размером около 156 МБ. Версия для Linux поддерживает CUDA (видеокарты NVIDIA).",
      pro_mac_tip: "Если macOS блокирует первый запуск, выполните в Терминале <code>xattr -dr com.apple.quarantine \"/Applications/TifMrcView Pro.app\"</code>.",
      pro_soon: "Установщики Pro скоро появятся."
    }
  };
  Object.keys(PRO).forEach(function (c) { for (var k in PRO[c]) T[c][k] = PRO[c][k]; });
  // Comparison tables and measured timings (home #compare, Pro page table).
  var CMP = {
    zh: {
      cmp_eye: '比一比', cmp_h: '<span>又小、又快，</span><span>实测为证。</span>',
      cmp_p: '同一台电脑、同一批文件，和 ImageJ / Fiji、IMOD 实测对比。',
      cr_free: '免费开源', cr_assoc: '原生安装包，双击图像就打开',
      cr_big: '数 GB 的栈不用整个读进内存，打开就能翻', cr_folder: '同目录缩略图，↑ ↓ 切换文件',
      cr_tiff: 'TIFF / OME-TIFF / ImageJ 栈', cr_mrc: 'MRC（冷冻电镜）',
      cr_conf: '共聚焦格式 <small>CZI · LSM · LIF · ND2 · OIB / OIR · IMS</small>',
      cr_em: '电镜格式 <small>DM3 / DM4 · SER · EMD · EER · TVIPS</small>',
      cr_ortho: 'XY / XZ / YZ 三视角', cr_meas: '按 nm / µm 测量，比例尺',
      cr_chan: '多通道分别开关、换色、调对比度', cr_mod: '自动叠加 IMOD 模型（.mod / .fid）',
      cr_fft: 'FFT 功率谱、衍射 1/nm 标尺', cr_spec: 'EELS / EDS 能谱查看', cr_upd: '自动更新',
      cr_android: '安卓版', cr_macro: '宏、脚本和批处理',
      cmp_plug: '插件', ck_y: "支持", ck_p: "部分支持，或要另找插件 / 手动设置", ck_n: '没有', ck_pl: '官方插件，在应用内一键安装',
      cmp_foot: '依据各软件官方文档和我们的实测（2026 年 10 月）。Fiji 和 IMOD 是功能全面的分析平台，宏脚本、海量插件和断层重建是它们的长项；TifMrcView 专注于又快又方便地看图。要做分析，请看 <a href="pro/">TifMrcView Pro</a>。',
      pcmp_h: '功能对比', pcmp_p: 'Pro 和基础版，以及常用的 ImageJ / Fiji、IMOD。', pcmp_basic: '基础版',
      pg_view: '看图', pg_3d: '3D 与分割', pg_em: '电镜分析',
      pr_fmt: 'TIFF、MRC 和各家显微镜、电镜厂商格式', pr_meas: '三视角、测量、FFT、能谱查看',
      pr_3d: '3D 体渲染 <small>MIP · 半透明 · 等值面</small>',
      pr_lab: '分割标签 <small>画笔 · 魔棒 · 阈值 · 分水岭 · 套索</small>',
      pr_mat: '材料：命名、配色、隐藏、锁定', pr_interp: '层间插值',
      pr_sam: 'AI 点击分割（SAM 2.1，本地运行）', pr_prop: '分割结果沿 Z 自动传播',
      pr_ctf: 'CTF 拟合', pr_drift: '漂移校正、对齐',
      pr_gpu: 'GPU 计算 <small>CUDA · Metal · DX12 · Vulkan</small>', pr_recon: '断层重建',
      pcmp_foot: '依据各软件官方文档（2026 年 10 月）。厂商格式在基础版和 Pro 里由官方插件提供。Fiji 和 IMOD 有大量插件和命令行程序，这里只比较装好就有的功能。',
      cr_lossless: "按原始精度显示（float32 / 16 位），不降成 8 位", cmp_plnote: "TifMrcView 的共聚焦和电镜厂商格式由官方插件提供，免费，在应用内一键安装。"
    },
    en: {
      cmp_eye: 'Compared', cmp_h: '<span>Small and fast.</span> <span>Measured.</span>',
      cmp_p: 'Same computer, same files: TifMrcView next to ImageJ / Fiji and IMOD.',
      cr_free: 'Free and open source', cr_assoc: 'Native installer, double-click an image to open it',
      cr_big: 'Browse multi-GB stacks without loading them all into memory', cr_folder: 'Folder thumbnails, ↑ ↓ to switch files',
      cr_tiff: 'TIFF / OME-TIFF / ImageJ stacks', cr_mrc: 'MRC (cryo-EM)',
      cr_conf: 'Confocal formats <small>CZI · LSM · LIF · ND2 · OIB / OIR · IMS</small>',
      cr_em: 'EM formats <small>DM3 / DM4 · SER · EMD · EER · TVIPS</small>',
      cr_ortho: 'XY / XZ / YZ views', cr_meas: 'Measure in nm / µm, scale bar',
      cr_chan: 'Per-channel on/off, colour and contrast', cr_mod: 'IMOD models (.mod / .fid) overlaid automatically',
      cr_fft: 'FFT power spectrum, 1/nm diffraction scale', cr_spec: 'EELS / EDS spectrum viewer', cr_upd: 'Automatic updates',
      cr_android: 'Android app', cr_macro: 'Macros, scripting and batch processing',
      cmp_plug: 'plugin', ck_y: "supported", ck_p: "partly, or needs a third-party plugin / manual setup", ck_n: 'no', ck_pl: 'official plugin, one-click install in the app',
      cmp_foot: 'Based on each program’s documentation and our own measurements (October 2026). Fiji and IMOD are full analysis platforms: macros, a huge plugin ecosystem and tomographic reconstruction are their strengths. TifMrcView focuses on viewing images quickly and easily. For analysis, see <a href="pro/">TifMrcView Pro</a>.',
      pcmp_h: 'Feature comparison', pcmp_p: 'Pro and the basic edition, next to ImageJ / Fiji and IMOD.', pcmp_basic: 'Basic',
      pg_view: 'Viewing', pg_3d: '3D and segmentation', pg_em: 'EM analysis',
      pr_fmt: 'TIFF, MRC and microscope / EM vendor formats', pr_meas: 'Orthogonal views, measuring, FFT, spectra',
      pr_3d: '3D volume rendering <small>MIP · translucent · isosurface</small>',
      pr_lab: 'Segmentation labels <small>brush · wand · threshold · watershed · lasso</small>',
      pr_mat: 'Materials: name, colour, hide, lock', pr_interp: 'Interpolation between slices',
      pr_sam: 'AI click segmentation (SAM 2.1, runs locally)', pr_prop: 'Propagate segmentation along Z',
      pr_ctf: 'CTF fitting', pr_drift: 'Drift correction and alignment',
      pr_gpu: 'GPU compute <small>CUDA · Metal · DX12 · Vulkan</small>', pr_recon: 'Tomographic reconstruction',
      pcmp_foot: 'Based on each program’s documentation (October 2026). Vendor formats come from official plugins in both editions. Fiji and IMOD have many plugins and command-line programs; this table compares what you get out of the box.',
      cr_lossless: "Shown at full precision (float32 / 16-bit), not reduced to 8-bit", cmp_plnote: "In TifMrcView, the confocal and EM vendor formats come from official plugins: free, one click to install in the app."
    },
    de: {
      cmp_eye: 'Im Vergleich', cmp_h: '<span>Klein und schnell.</span> <span>Gemessen.</span>',
      cmp_p: 'Gleicher Rechner, gleiche Dateien: TifMrcView neben ImageJ / Fiji und IMOD.',
      cr_free: 'Kostenlos und Open Source', cr_assoc: 'Natives Installationspaket, Bild per Doppelklick öffnen',
      cr_big: 'Stapel mit mehreren GB durchblättern, ohne alles in den Speicher zu laden', cr_folder: 'Ordner-Miniaturen, ↑ ↓ wechselt die Datei',
      cr_tiff: 'TIFF / OME-TIFF / ImageJ-Stapel', cr_mrc: 'MRC (Kryo-EM)',
      cr_conf: 'Konfokalformate <small>CZI · LSM · LIF · ND2 · OIB / OIR · IMS</small>',
      cr_em: 'EM-Formate <small>DM3 / DM4 · SER · EMD · EER · TVIPS</small>',
      cr_ortho: 'XY- / XZ- / YZ-Ansicht', cr_meas: 'Messen in nm / µm, Maßstab',
      cr_chan: 'Kanäle einzeln ein/aus, Farbe und Kontrast', cr_mod: 'IMOD-Modelle (.mod / .fid) automatisch eingeblendet',
      cr_fft: 'FFT-Leistungsspektrum, 1/nm-Beugungsskala', cr_spec: 'EELS- / EDS-Spektrenansicht', cr_upd: 'Automatische Updates',
      cr_android: 'Android-App', cr_macro: 'Makros, Skripte und Stapelverarbeitung',
      cmp_plug: 'Plugin', ck_y: "unterstützt", ck_p: "teilweise, oder mit Fremd-Plugin / manueller Einstellung", ck_n: 'nein', ck_pl: 'offizielles Plugin, mit einem Klick in der App installiert',
      cmp_foot: 'Nach der Dokumentation der Programme und eigenen Messungen (Oktober 2026). Fiji und IMOD sind vollständige Analyseplattformen: Makros, unzählige Plugins und tomografische Rekonstruktion sind ihre Stärken. TifMrcView konzentriert sich darauf, Bilder schnell und bequem anzusehen. Für Analysen siehe <a href="pro/">TifMrcView Pro</a>.',
      pcmp_h: 'Funktionsvergleich', pcmp_p: 'Pro und die Basisversion, neben ImageJ / Fiji und IMOD.', pcmp_basic: 'Basis',
      pg_view: 'Ansehen', pg_3d: '3D und Segmentierung', pg_em: 'EM-Analyse',
      pr_fmt: 'TIFF, MRC und Formate der Mikroskop- und EM-Hersteller', pr_meas: 'Orthogonalansichten, Messen, FFT, Spektren',
      pr_3d: '3D-Volumendarstellung <small>MIP · transparent · Isofläche</small>',
      pr_lab: 'Segmentierungs-Labels <small>Pinsel · Zauberstab · Schwelle · Watershed · Lasso</small>',
      pr_mat: 'Materialien: benennen, färben, ausblenden, sperren', pr_interp: 'Interpolation zwischen Schichten',
      pr_sam: 'KI-Segmentierung per Klick (SAM 2.1, lokal)', pr_prop: 'Segmentierung entlang Z fortsetzen',
      pr_ctf: 'CTF-Anpassung', pr_drift: 'Driftkorrektur und Ausrichtung',
      pr_gpu: 'GPU-Berechnung <small>CUDA · Metal · DX12 · Vulkan</small>', pr_recon: 'Tomografische Rekonstruktion',
      pcmp_foot: 'Nach der Dokumentation der Programme (Oktober 2026). Herstellerformate kommen in beiden Versionen aus offiziellen Plugins. Fiji und IMOD haben viele Plugins und Kommandozeilenprogramme; verglichen wird, was nach der Installation dabei ist.',
      cr_lossless: "Anzeige in voller Genauigkeit (float32 / 16 Bit), nicht auf 8 Bit reduziert", cmp_plnote: "In TifMrcView kommen die Konfokal- und EM-Herstellerformate aus offiziellen Plugins: kostenlos, mit einem Klick in der App installiert."
    },
    ja: {
      cmp_eye: '比較', cmp_h: '<span>小さく、速い。</span><span>実測で。</span>',
      cmp_p: '同じコンピューター、同じファイルで、ImageJ / Fiji、IMOD と実測比較しました。',
      cr_free: '無料・オープンソース', cr_assoc: 'ネイティブのインストーラー、画像をダブルクリックで開く',
      cr_big: '数 GB のスタックを全部メモリに読み込まずに閲覧', cr_folder: 'フォルダーのサムネイル、↑ ↓ でファイル切り替え',
      cr_tiff: 'TIFF / OME-TIFF / ImageJ スタック', cr_mrc: 'MRC（クライオ電顕）',
      cr_conf: '共焦点フォーマット <small>CZI · LSM · LIF · ND2 · OIB / OIR · IMS</small>',
      cr_em: '電顕フォーマット <small>DM3 / DM4 · SER · EMD · EER · TVIPS</small>',
      cr_ortho: 'XY / XZ / YZ ビュー', cr_meas: 'nm / µm で計測、スケールバー',
      cr_chan: 'チャンネルごとの表示切替・色・コントラスト', cr_mod: 'IMOD モデル（.mod / .fid）を自動で重ねて表示',
      cr_fft: 'FFT パワースペクトル、回折の 1/nm スケール', cr_spec: 'EELS / EDS スペクトルビューア', cr_upd: '自動アップデート',
      cr_android: 'Android 版', cr_macro: 'マクロ・スクリプト・バッチ処理',
      cmp_plug: 'プラグイン', ck_y: "対応", ck_p: "一部対応、または別途プラグイン / 手動設定が必要", ck_n: 'なし', ck_pl: '公式プラグイン、アプリ内でワンクリック導入',
      cmp_foot: '各ソフトの公式ドキュメントと私たちの実測（2026 年 10 月）に基づきます。Fiji と IMOD は総合的な解析プラットフォームで、マクロ、膨大なプラグイン、トモグラム再構成が強みです。TifMrcView は画像を速く手軽に見ることに特化しています。解析には <a href="pro/">TifMrcView Pro</a> をどうぞ。',
      pcmp_h: '機能比較', pcmp_p: 'Pro と基本版、そして ImageJ / Fiji、IMOD。', pcmp_basic: '基本版',
      pg_view: '閲覧', pg_3d: '3D とセグメンテーション', pg_em: '電顕解析',
      pr_fmt: 'TIFF、MRC、各社の顕微鏡・電顕フォーマット', pr_meas: '直交ビュー、計測、FFT、スペクトル',
      pr_3d: '3D ボリュームレンダリング <small>MIP · 半透明 · 等値面</small>',
      pr_lab: 'セグメンテーションラベル <small>ブラシ · 魔法の杖 · しきい値 · 分水嶺 · 投げ縄</small>',
      pr_mat: 'マテリアル：名前・色・非表示・ロック', pr_interp: 'スライス間補間',
      pr_sam: 'AI クリックセグメンテーション（SAM 2.1、ローカル実行）', pr_prop: 'セグメンテーションを Z 方向に自動伝播',
      pr_ctf: 'CTF フィッティング', pr_drift: 'ドリフト補正・位置合わせ',
      pr_gpu: 'GPU 計算 <small>CUDA · Metal · DX12 · Vulkan</small>', pr_recon: 'トモグラム再構成',
      pcmp_foot: '各ソフトの公式ドキュメント（2026 年 10 月）に基づきます。各社フォーマットはどちらの版でも公式プラグインで対応します。Fiji と IMOD には多数のプラグインやコマンドラインプログラムがありますが、ここではインストール直後に使える機能を比べています。',
      cr_lossless: "元の精度（float32 / 16 ビット）で表示、8 ビットに落とさない", cmp_plnote: "TifMrcView の共焦点・電顕メーカーフォーマットは公式プラグインで対応します。無料で、アプリ内からワンクリックで導入できます。"
    },
    es: {
      cmp_eye: 'Comparativa', cmp_h: '<span>Pequeño y rápido.</span> <span>Medido.</span>',
      cmp_p: 'Mismo ordenador, mismos archivos: TifMrcView junto a ImageJ / Fiji e IMOD.',
      cr_free: 'Gratis y de código abierto', cr_assoc: 'Instalador nativo; doble clic en una imagen para abrirla',
      cr_big: 'Recorre pilas de varios GB sin cargarlas enteras en memoria', cr_folder: 'Miniaturas de la carpeta, ↑ ↓ cambia de archivo',
      cr_tiff: 'TIFF / OME-TIFF / pilas de ImageJ', cr_mrc: 'MRC (crio-EM)',
      cr_conf: 'Formatos confocales <small>CZI · LSM · LIF · ND2 · OIB / OIR · IMS</small>',
      cr_em: 'Formatos de ME <small>DM3 / DM4 · SER · EMD · EER · TVIPS</small>',
      cr_ortho: 'Vistas XY / XZ / YZ', cr_meas: 'Medición en nm / µm, barra de escala',
      cr_chan: 'Canales por separado: activar, color y contraste', cr_mod: 'Modelos de IMOD (.mod / .fid) superpuestos automáticamente',
      cr_fft: 'Espectro de potencia FFT, escala de difracción en 1/nm', cr_spec: 'Visor de espectros EELS / EDS', cr_upd: 'Actualizaciones automáticas',
      cr_android: 'App para Android', cr_macro: 'Macros, scripts y procesamiento por lotes',
      cmp_plug: 'plugin', ck_y: "compatible", ck_p: "en parte, o requiere un plugin de terceros / ajuste manual", ck_n: 'no', ck_pl: 'plugin oficial, se instala con un clic en la app',
      cmp_foot: 'Según la documentación de cada programa y nuestras propias mediciones (octubre de 2026). Fiji e IMOD son plataformas de análisis completas: las macros, su enorme ecosistema de plugins y la reconstrucción tomográfica son sus puntos fuertes. TifMrcView se centra en ver imágenes de forma rápida y cómoda. Para análisis, consulta <a href="pro/">TifMrcView Pro</a>.',
      pcmp_h: 'Comparativa de funciones', pcmp_p: 'Pro y la edición básica, junto a ImageJ / Fiji e IMOD.', pcmp_basic: 'Básica',
      pg_view: 'Visualización', pg_3d: '3D y segmentación', pg_em: 'Análisis de ME',
      pr_fmt: 'TIFF, MRC y formatos de fabricantes de microscopios y ME', pr_meas: 'Vistas ortogonales, medición, FFT, espectros',
      pr_3d: 'Renderizado 3D de volumen <small>MIP · translúcido · isosuperficie</small>',
      pr_lab: 'Etiquetas de segmentación <small>pincel · varita · umbral · watershed · lazo</small>',
      pr_mat: 'Materiales: nombre, color, ocultar, bloquear', pr_interp: 'Interpolación entre cortes',
      pr_sam: 'Segmentación con IA por clic (SAM 2.1, en local)', pr_prop: 'Propagar la segmentación a lo largo de Z',
      pr_ctf: 'Ajuste de CTF', pr_drift: 'Corrección de deriva y alineación',
      pr_gpu: 'Cálculo en GPU <small>CUDA · Metal · DX12 · Vulkan</small>', pr_recon: 'Reconstrucción tomográfica',
      pcmp_foot: 'Según la documentación de cada programa (octubre de 2026). En ambas ediciones los formatos de fabricantes llegan con plugins oficiales. Fiji e IMOD tienen muchos plugins y programas de línea de comandos; aquí se compara lo que trae cada uno al instalarlo.',
      cr_lossless: "Se muestra con la precisión original (float32 / 16 bits), sin reducir a 8 bits", cmp_plnote: "En TifMrcView, los formatos de fabricantes confocales y de ME llegan con plugins oficiales: gratis y se instalan con un clic en la app."
    },
    fr: {
      cmp_eye: 'Comparaison', cmp_h: '<span>Léger et rapide.</span> <span>Mesuré.</span>',
      cmp_p: 'Même ordinateur, mêmes fichiers : TifMrcView face à ImageJ / Fiji et IMOD.',
      cr_free: 'Gratuit et open source', cr_assoc: 'Installeur natif, double-clic sur une image pour l’ouvrir',
      cr_big: 'Parcourir des piles de plusieurs Go sans tout charger en mémoire', cr_folder: 'Miniatures du dossier, ↑ ↓ pour changer de fichier',
      cr_tiff: 'TIFF / OME-TIFF / piles ImageJ', cr_mrc: 'MRC (cryo-ME)',
      cr_conf: 'Formats confocaux <small>CZI · LSM · LIF · ND2 · OIB / OIR · IMS</small>',
      cr_em: 'Formats de ME <small>DM3 / DM4 · SER · EMD · EER · TVIPS</small>',
      cr_ortho: 'Vues XY / XZ / YZ', cr_meas: 'Mesures en nm / µm, barre d’échelle',
      cr_chan: 'Canaux séparés : affichage, couleur et contraste', cr_mod: 'Modèles IMOD (.mod / .fid) superposés automatiquement',
      cr_fft: 'Spectre de puissance FFT, échelle de diffraction en 1/nm', cr_spec: 'Visionneuse de spectres EELS / EDS', cr_upd: 'Mises à jour automatiques',
      cr_android: 'Application Android', cr_macro: 'Macros, scripts et traitement par lots',
      cmp_plug: 'plugin', ck_y: "pris en charge", ck_p: "en partie, ou avec un plugin tiers / réglage manuel", ck_n: 'non', ck_pl: 'plugin officiel, installé en un clic dans l’app',
      cmp_foot: 'D’après la documentation de chaque logiciel et nos propres mesures (octobre 2026). Fiji et IMOD sont des plateformes d’analyse complètes : macros, immense écosystème de plugins et reconstruction tomographique sont leurs points forts. TifMrcView se concentre sur la visualisation rapide et simple. Pour l’analyse, voir <a href="pro/">TifMrcView Pro</a>.',
      pcmp_h: 'Comparaison des fonctions', pcmp_p: 'Pro et l’édition de base, face à ImageJ / Fiji et IMOD.', pcmp_basic: 'Base',
      pg_view: 'Visualisation', pg_3d: '3D et segmentation', pg_em: 'Analyse de ME',
      pr_fmt: 'TIFF, MRC et formats des fabricants de microscopes et de ME', pr_meas: 'Vues orthogonales, mesures, FFT, spectres',
      pr_3d: 'Rendu de volume 3D <small>MIP · translucide · isosurface</small>',
      pr_lab: 'Étiquettes de segmentation <small>pinceau · baguette · seuil · watershed · lasso</small>',
      pr_mat: 'Matériaux : nom, couleur, masquer, verrouiller', pr_interp: 'Interpolation entre coupes',
      pr_sam: 'Segmentation IA au clic (SAM 2.1, en local)', pr_prop: 'Propager la segmentation le long de Z',
      pr_ctf: 'Ajustement de la CTF', pr_drift: 'Correction de dérive et alignement',
      pr_gpu: 'Calcul sur GPU <small>CUDA · Metal · DX12 · Vulkan</small>', pr_recon: 'Reconstruction tomographique',
      pcmp_foot: 'D’après la documentation de chaque logiciel (octobre 2026). Dans les deux éditions, les formats des fabricants viennent de plugins officiels. Fiji et IMOD ont de nombreux plugins et programmes en ligne de commande ; on compare ici ce qui est disponible dès l’installation.',
      cr_lossless: "Affichage en précision d’origine (float32 / 16 bits), sans réduction à 8 bits", cmp_plnote: "Dans TifMrcView, les formats des fabricants (confocal et ME) viennent de plugins officiels : gratuits, installés en un clic dans l’app."
    },
    ru: {
      cmp_eye: 'Сравнение', cmp_h: '<span>Маленький и быстрый.</span> <span>Измерено.</span>',
      cmp_p: 'Один компьютер, одни и те же файлы: TifMrcView рядом с ImageJ / Fiji и IMOD.',
      cr_free: 'Бесплатно, открытый код', cr_assoc: 'Нативный установщик, изображение открывается двойным щелчком',
      cr_big: 'Листать стеки в несколько ГБ, не загружая их целиком в память', cr_folder: 'Миниатюры папки, ↑ ↓ переключают файлы',
      cr_tiff: 'TIFF / OME-TIFF / стеки ImageJ', cr_mrc: 'MRC (крио-ЭМ)',
      cr_conf: 'Конфокальные форматы <small>CZI · LSM · LIF · ND2 · OIB / OIR · IMS</small>',
      cr_em: 'Форматы ЭМ <small>DM3 / DM4 · SER · EMD · EER · TVIPS</small>',
      cr_ortho: 'Проекции XY / XZ / YZ', cr_meas: 'Измерения в нм / мкм, масштабная линейка',
      cr_chan: 'Каналы по отдельности: показ, цвет, контраст', cr_mod: 'Модели IMOD (.mod / .fid) накладываются автоматически',
      cr_fft: 'Спектр мощности FFT, шкала дифракции в 1/нм', cr_spec: 'Просмотр спектров EELS / EDS', cr_upd: 'Автообновление',
      cr_android: 'Приложение для Android', cr_macro: 'Макросы, скрипты и пакетная обработка',
      cmp_plug: 'плагин', ck_y: "есть", ck_p: "частично, или нужен сторонний плагин / ручная настройка", ck_n: 'нет', ck_pl: 'официальный плагин, ставится в приложении одним щелчком',
      cmp_foot: 'По документации программ и нашим собственным измерениям (октябрь 2026). Fiji и IMOD — полноценные платформы анализа: макросы, огромное число плагинов и томографическая реконструкция — их сильные стороны. TifMrcView сосредоточен на быстром и удобном просмотре. Для анализа см. <a href="pro/">TifMrcView Pro</a>.',
      pcmp_h: 'Сравнение возможностей', pcmp_p: 'Pro и базовая версия рядом с ImageJ / Fiji и IMOD.', pcmp_basic: 'Базовая',
      pg_view: 'Просмотр', pg_3d: '3D и сегментация', pg_em: 'Анализ ЭМ',
      pr_fmt: 'TIFF, MRC и форматы производителей микроскопов и ЭМ', pr_meas: 'Ортогональные проекции, измерения, FFT, спектры',
      pr_3d: '3D-рендеринг объёма <small>MIP · полупрозрачный · изоповерхность</small>',
      pr_lab: 'Метки сегментации <small>кисть · волшебная палочка · порог · водораздел · лассо</small>',
      pr_mat: 'Материалы: имя, цвет, скрытие, блокировка', pr_interp: 'Интерполяция между срезами',
      pr_sam: 'ИИ-сегментация щелчком (SAM 2.1, локально)', pr_prop: 'Распространение сегментации по Z',
      pr_ctf: 'Подгонка CTF', pr_drift: 'Коррекция дрейфа и выравнивание',
      pr_gpu: 'Вычисления на GPU <small>CUDA · Metal · DX12 · Vulkan</small>', pr_recon: 'Томографическая реконструкция',
      pcmp_foot: 'По документации программ (октябрь 2026). Форматы производителей в обеих версиях дают официальные плагины. У Fiji и IMOD много плагинов и программ командной строки; здесь сравнивается то, что есть сразу после установки.',
      cr_lossless: "Показ с исходной точностью (float32 / 16 бит), без понижения до 8 бит", cmp_plnote: "В TifMrcView конфокальные и ЭМ-форматы производителей дают официальные плагины: бесплатно, ставятся в приложении одним щелчком."
    }
  };
  Object.keys(CMP).forEach(function (c) { for (var k in CMP[c]) T[c][k] = CMP[c][k]; });
  // Measured timings (MacBook Pro M1 Max, October 2026; raw runs kept outside the repo).
  var BENCH = {
    zh: {
      bt_dl: "下载大小",
      bs_dl: "macOS 安装包。Fiji 为官方 Apple 芯片版（自带 Java）。",
      bt_start: "冷启动",
      bs_start: "从启动到窗口出现，不打开文件。",
      bt_mrc: "启动并打开 2 GB 的 MRC",
      bs_mrc: "1024 × 1024 × 512，float32。程序没在运行时双击文件，到第一张图出现，含程序启动。",
      bt_ome: "启动并打开压缩的 OME-TIFF",
      bs_ome: "蔡司 Airyscan Z 栈，41 层，Deflate 压缩，100 MB。同样含程序启动。",
      bt_mem: "打开 2 GB MRC 后的内存",
      bs_mem: "所有进程的物理内存合计。TifMrcView 不会一次读入整个文件，而是先显示当前层，再在后台预读附近的层，最多用内存的 1/4；这台 32 GB 的电脑放得下整个文件，所以全部预读了。3dmod 默认把数据转成 8 位，所以更少。",
      bt_cpu: "空闲时的 CPU 占用",
      bs_cpu: "文件打开后、不操作时的 30 秒平均值。",
      bench_m: "测试机：MacBook Pro（M1 Max，32 GB 内存，macOS 27），2026 年 10 月。版本：TifMrcView 0.7.63、Fiji（ImageJ 2.16.0 / 1.54p）、IMOD 5.1.13。每项取 2–6 次运行的中位数，磁盘缓存是热的，越短越好。打开时间都从启动程序算起（TifMrcView 自身启动约 0.7 秒）。TifMrcView 打开时只读当前层，其余层在后台预读，不影响翻看；Fiji 要整栈读完才显示；3dmod 显示的是转成 8 位的数据，TifMrcView 保留 float32 原始数值。",
      pbench_h: "同样很快",
      pbench_p: "Pro 在同一台电脑上，和 Fiji、IMOD 实测对比。",
      pbench_m: "测试机：MacBook Pro（M1 Max，32 GB 内存，macOS 27），2026 年 10 月。TifMrcView Pro 0.7.56、Fiji（ImageJ 2.16.0 / 1.54p）、IMOD 5.1.13。每项取多次运行的中位数，磁盘缓存是热的，越短越好，打开时间含程序启动。Pro 的内存里包括后台预读的层（最多用内存的 1/4）；3dmod 默认把数据转成 8 位，所以内存更少。"
    },
    en: {
      bt_dl: "Download size",
      bs_dl: "macOS package. Fiji: the official Apple silicon build with bundled Java.",
      bt_start: "Cold start",
      bs_start: "From launch to the window appearing, no file.",
      bt_mrc: "Launch and open a 2 GB MRC",
      bs_mrc: "1024 × 1024 × 512, float32. Double-click with the app not running, until the first image; includes app start.",
      bt_ome: "Launch and open a compressed OME-TIFF",
      bs_ome: "Zeiss Airyscan Z stack, 41 slices, Deflate, 100 MB. Includes app start.",
      bt_mem: "Memory with the 2 GB MRC open",
      bs_mem: "Physical memory of all processes. TifMrcView never loads the whole file up front: it shows the current slice, then reads nearby slices in the background, using at most a quarter of RAM. On this 32 GB machine the whole file fits, so all of it was read ahead. 3dmod converts to 8-bit by default, hence less.",
      bt_cpu: "CPU use when idle",
      bs_cpu: "30-second average with the file open and nothing happening.",
      bench_m: "Test machine: MacBook Pro (M1 Max, 32 GB RAM, macOS 27), October 2026. Versions: TifMrcView 0.7.63, Fiji (ImageJ 2.16.0 / 1.54p), IMOD 5.1.13. Median of 2–6 runs each, warm disk cache; shorter is better. Open times start at app launch (TifMrcView itself starts in about 0.7 s). TifMrcView reads only the current slice to open; the rest is read ahead in the background without getting in the way. Fiji shows nothing until the whole stack is read; 3dmod shows data converted to 8-bit, while TifMrcView keeps the original float32 values.",
      pbench_h: "Just as fast",
      pbench_p: "Pro on the same computer, next to Fiji and IMOD.",
      pbench_m: "Test machine: MacBook Pro (M1 Max, 32 GB RAM, macOS 27), October 2026. TifMrcView Pro 0.7.56, Fiji (ImageJ 2.16.0 / 1.54p), IMOD 5.1.13. Median of several runs, warm disk cache; shorter is better; open times include app start. Pro’s memory includes slices read ahead in the background (at most a quarter of RAM); 3dmod converts to 8-bit by default, hence its lower memory."
    },
    de: {
      bt_dl: "Downloadgröße",
      bs_dl: "macOS-Paket. Fiji: offizielle Apple-Silicon-Version mit Java.",
      bt_start: "Kaltstart",
      bs_start: "Vom Start bis zum Fenster, ohne Datei.",
      bt_mrc: "Starten und 2-GB-MRC öffnen",
      bs_mrc: "1024 × 1024 × 512, float32. Doppelklick bei geschlossener App bis zum ersten Bild, inklusive App-Start.",
      bt_ome: "Starten und komprimiertes OME-TIFF öffnen",
      bs_ome: "Zeiss-Airyscan-Z-Stapel, 41 Schichten, Deflate, 100 MB. Inklusive App-Start.",
      bt_mem: "Speicher mit geöffnetem 2-GB-MRC",
      bs_mem: "Physischer Speicher aller Prozesse. TifMrcView lädt nie die ganze Datei auf einmal: Es zeigt die aktuelle Schicht und liest benachbarte Schichten im Hintergrund vor, höchstens ein Viertel des RAM. Auf diesem 32-GB-Rechner passt die ganze Datei hinein, daher wurde alles vorgelesen. 3dmod wandelt standardmäßig in 8 Bit um, daher weniger.",
      bt_cpu: "CPU-Last im Leerlauf",
      bs_cpu: "Mittel über 30 Sekunden bei geöffneter Datei ohne Bedienung.",
      bench_m: "Testrechner: MacBook Pro (M1 Max, 32 GB RAM, macOS 27), Oktober 2026. Versionen: TifMrcView 0.7.63, Fiji (ImageJ 2.16.0 / 1.54p), IMOD 5.1.13. Median aus 2–6 Läufen, warmer Plattencache; kürzer ist besser. Öffnungszeiten zählen ab dem App-Start (TifMrcView selbst startet in etwa 0,7 s). TifMrcView liest zum Öffnen nur die aktuelle Schicht, der Rest wird im Hintergrund vorgelesen, ohne zu stören. Fiji zeigt erst etwas, wenn der ganze Stapel gelesen ist; 3dmod zeigt in 8 Bit umgewandelte Daten, TifMrcView behält die originalen float32-Werte.",
      pbench_h: "Genauso schnell",
      pbench_p: "Pro auf demselben Rechner, neben Fiji und IMOD.",
      pbench_m: "Testrechner: MacBook Pro (M1 Max, 32 GB RAM, macOS 27), Oktober 2026. TifMrcView Pro 0.7.56, Fiji (ImageJ 2.16.0 / 1.54p), IMOD 5.1.13. Median mehrerer Läufe, warmer Plattencache; kürzer ist besser; Öffnungszeiten inklusive App-Start. Der Speicher von Pro enthält im Hintergrund vorgelesene Schichten (höchstens ein Viertel des RAM); 3dmod wandelt standardmäßig in 8 Bit um, daher weniger Speicher."
    },
    ja: {
      bt_dl: "ダウンロードサイズ",
      bs_dl: "macOS 版パッケージ。Fiji は Java 同梱の公式 Apple シリコン版。",
      bt_start: "コールドスタート",
      bs_start: "起動からウィンドウ表示まで（ファイルなし）。",
      bt_mrc: "起動して 2 GB の MRC を開く",
      bs_mrc: "1024 × 1024 × 512、float32。アプリ未起動の状態でダブルクリックし、最初の画像が出るまで（起動時間を含む）。",
      bt_ome: "起動して圧縮 OME-TIFF を開く",
      bs_ome: "Zeiss Airyscan Z スタック、41 枚、Deflate 圧縮、100 MB。起動時間を含む。",
      bt_mem: "2 GB MRC を開いた後のメモリ",
      bs_mem: "全プロセスの物理メモリ合計。TifMrcView はファイル全体を一度に読み込みません。現在のスライスを表示してから、近くのスライスをバックグラウンドで先読みします（上限はメモリの 1/4）。この 32 GB の機械ではファイル全体が収まるため、すべて先読みされました。3dmod は既定で 8 ビットに変換するため少なめ。",
      bt_cpu: "アイドル時の CPU 使用率",
      bs_cpu: "ファイルを開いて何も操作しない 30 秒間の平均。",
      bench_m: "テスト機：MacBook Pro（M1 Max、メモリ 32 GB、macOS 27）、2026 年 10 月。バージョン：TifMrcView 0.7.63、Fiji（ImageJ 2.16.0 / 1.54p）、IMOD 5.1.13。各 2～6 回の中央値、ディスクキャッシュは温まった状態、短いほど良好。開く時間はアプリの起動から計測（TifMrcView 自体の起動は約 0.7 秒）。TifMrcView は開くときに現在のスライスだけを読み、残りは操作の邪魔をせずにバックグラウンドで先読みします。Fiji はスタック全体を読み終えるまで表示しません。3dmod は 8 ビットに変換したデータを表示し、TifMrcView は元の float32 値を保ちます。",
      pbench_h: "Pro も速い",
      pbench_p: "同じコンピューターで Fiji、IMOD と実測比較。",
      pbench_m: "テスト機：MacBook Pro（M1 Max、メモリ 32 GB、macOS 27）、2026 年 10 月。TifMrcView Pro 0.7.56、Fiji（ImageJ 2.16.0 / 1.54p）、IMOD 5.1.13。複数回の中央値、ディスクキャッシュは温まった状態、短いほど良好、開く時間は起動を含みます。Pro のメモリにはバックグラウンドで先読みしたスライス（上限はメモリの 1/4）が含まれます。3dmod は既定で 8 ビットに変換するためメモリが少なめです。"
    },
    es: {
      bt_dl: "Tamaño de descarga",
      bs_dl: "Paquete para macOS. Fiji: versión oficial para Apple silicon con Java incluido.",
      bt_start: "Arranque en frío",
      bs_start: "Desde el inicio hasta que aparece la ventana, sin archivo.",
      bt_mrc: "Iniciar y abrir un MRC de 2 GB",
      bs_mrc: "1024 × 1024 × 512, float32. Doble clic con la app cerrada hasta la primera imagen; incluye el arranque.",
      bt_ome: "Iniciar y abrir un OME-TIFF comprimido",
      bs_ome: "Pila Z de Zeiss Airyscan, 41 cortes, Deflate, 100 MB. Incluye el arranque.",
      bt_mem: "Memoria con el MRC de 2 GB abierto",
      bs_mem: "Memoria física de todos los procesos. TifMrcView nunca carga el archivo entero de golpe: muestra el corte actual y lee por adelantado los cercanos en segundo plano, usando como mucho una cuarta parte de la RAM. En este equipo de 32 GB cabe el archivo completo, así que se leyó entero. 3dmod convierte a 8 bits por defecto, por eso usa menos.",
      bt_cpu: "Uso de CPU en reposo",
      bs_cpu: "Media de 30 segundos con el archivo abierto y sin tocar nada.",
      bench_m: "Equipo de prueba: MacBook Pro (M1 Max, 32 GB de RAM, macOS 27), octubre de 2026. Versiones: TifMrcView 0.7.63, Fiji (ImageJ 2.16.0 / 1.54p), IMOD 5.1.13. Mediana de 2–6 ejecuciones, caché de disco caliente; menos es mejor. Los tiempos de apertura cuentan desde el arranque de la app (TifMrcView arranca en unos 0,7 s). TifMrcView solo lee el corte actual para abrir; el resto se lee por adelantado en segundo plano sin estorbar. Fiji no muestra nada hasta leer toda la pila; 3dmod muestra los datos convertidos a 8 bits, mientras que TifMrcView conserva los valores float32 originales.",
      pbench_h: "Igual de rápido",
      pbench_p: "Pro en el mismo ordenador, junto a Fiji e IMOD.",
      pbench_m: "Equipo de prueba: MacBook Pro (M1 Max, 32 GB de RAM, macOS 27), octubre de 2026. TifMrcView Pro 0.7.56, Fiji (ImageJ 2.16.0 / 1.54p), IMOD 5.1.13. Mediana de varias ejecuciones, caché de disco caliente; menos es mejor; los tiempos de apertura incluyen el arranque. La memoria de Pro incluye cortes leídos por adelantado (como mucho una cuarta parte de la RAM); 3dmod convierte a 8 bits por defecto, por eso usa menos memoria."
    },
    fr: {
      bt_dl: "Taille du téléchargement",
      bs_dl: "Paquet macOS. Fiji : version officielle Apple silicon avec Java intégré.",
      bt_start: "Démarrage à froid",
      bs_start: "Du lancement à l’apparition de la fenêtre, sans fichier.",
      bt_mrc: "Lancer et ouvrir un MRC de 2 Go",
      bs_mrc: "1024 × 1024 × 512, float32. Double-clic, app fermée, jusqu’à la première image ; démarrage compris.",
      bt_ome: "Lancer et ouvrir un OME-TIFF compressé",
      bs_ome: "Pile Z Zeiss Airyscan, 41 coupes, Deflate, 100 Mo. Démarrage compris.",
      bt_mem: "Mémoire avec le MRC de 2 Go ouvert",
      bs_mem: "Mémoire physique de tous les processus. TifMrcView ne charge jamais tout le fichier d’un coup : il affiche la coupe courante puis lit les coupes voisines en arrière-plan, avec au plus un quart de la RAM. Sur cette machine de 32 Go, le fichier entier tient, il a donc été lu en entier. 3dmod convertit en 8 bits par défaut, d’où moins.",
      bt_cpu: "CPU au repos",
      bs_cpu: "Moyenne sur 30 secondes, fichier ouvert, sans rien faire.",
      bench_m: "Machine de test : MacBook Pro (M1 Max, 32 Go de RAM, macOS 27), octobre 2026. Versions : TifMrcView 0.7.63, Fiji (ImageJ 2.16.0 / 1.54p), IMOD 5.1.13. Médiane de 2 à 6 exécutions, cache disque chaud ; plus court est meilleur. Les temps d’ouverture comptent à partir du lancement (TifMrcView démarre en 0,7 s environ). Pour ouvrir, TifMrcView ne lit que la coupe courante ; le reste est lu en arrière-plan sans gêner. Fiji n’affiche rien avant d’avoir lu toute la pile ; 3dmod affiche des données converties en 8 bits, alors que TifMrcView garde les valeurs float32 d’origine.",
      pbench_h: "Tout aussi rapide",
      pbench_p: "Pro sur le même ordinateur, face à Fiji et IMOD.",
      pbench_m: "Machine de test : MacBook Pro (M1 Max, 32 Go de RAM, macOS 27), octobre 2026. TifMrcView Pro 0.7.56, Fiji (ImageJ 2.16.0 / 1.54p), IMOD 5.1.13. Médiane de plusieurs exécutions, cache disque chaud ; plus court est meilleur ; temps d’ouverture démarrage compris. La mémoire de Pro inclut les coupes lues en arrière-plan (au plus un quart de la RAM) ; 3dmod convertit en 8 bits par défaut, d’où sa mémoire plus faible."
    },
    ru: {
      bt_dl: "Размер загрузки",
      bs_dl: "Пакет для macOS. Fiji — официальная сборка для Apple silicon со встроенной Java.",
      bt_start: "Холодный запуск",
      bs_start: "От запуска до появления окна, без файла.",
      bt_mrc: "Запуск и открытие MRC на 2 ГБ",
      bs_mrc: "1024 × 1024 × 512, float32. Двойной щелчок при закрытом приложении до первого изображения, включая запуск.",
      bt_ome: "Запуск и открытие сжатого OME-TIFF",
      bs_ome: "Z-стек Zeiss Airyscan, 41 срез, Deflate, 100 МБ. Включая запуск.",
      bt_mem: "Память с открытым MRC на 2 ГБ",
      bs_mem: "Физическая память всех процессов. TifMrcView никогда не загружает весь файл сразу: показывает текущий срез и в фоне читает соседние, занимая не больше четверти ОЗУ. На этом компьютере с 32 ГБ файл помещается целиком, поэтому прочитан весь. 3dmod по умолчанию переводит данные в 8 бит, поэтому меньше.",
      bt_cpu: "Загрузка CPU в простое",
      bs_cpu: "Среднее за 30 секунд с открытым файлом без действий.",
      bench_m: "Тестовый компьютер: MacBook Pro (M1 Max, 32 ГБ ОЗУ, macOS 27), октябрь 2026. Версии: TifMrcView 0.7.63, Fiji (ImageJ 2.16.0 / 1.54p), IMOD 5.1.13. Медиана 2–6 запусков, дисковый кэш прогрет; меньше — лучше. Время открытия считается с запуска приложения (сам TifMrcView запускается примерно за 0,7 с). Чтобы открыть файл, TifMrcView читает только текущий срез; остальное читается в фоне и не мешает работе. Fiji ничего не показывает, пока не прочитает весь стек; 3dmod показывает данные, переведённые в 8 бит, а TifMrcView сохраняет исходные значения float32.",
      pbench_h: "Так же быстро",
      pbench_p: "Pro на том же компьютере рядом с Fiji и IMOD.",
      pbench_m: "Тестовый компьютер: MacBook Pro (M1 Max, 32 ГБ ОЗУ, macOS 27), октябрь 2026. TifMrcView Pro 0.7.56, Fiji (ImageJ 2.16.0 / 1.54p), IMOD 5.1.13. Медиана нескольких запусков, дисковый кэш прогрет; меньше — лучше; время открытия включает запуск. Память Pro включает срезы, прочитанные в фоне (не больше четверти ОЗУ); 3dmod по умолчанию переводит данные в 8 бит, поэтому памяти нужно меньше."
    }
  };
  Object.keys(BENCH).forEach(function (c) { for (var k in BENCH[c]) T[c][k] = BENCH[c][k]; });

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
  var os = /iPhone|iPad|iPod|Android/.test(ua) ? null : /Mac/.test(ua) ? 'mac' : /Win/.test(ua) ? 'win' : /Linux|X11/.test(ua) ? 'linux' : null;
  var BASE = document.documentElement.getAttribute('data-base') || '';
  // Downloads go through get/, which the server counts and redirects to the cached file.
  var DL = 'get/';
  var HOME = !!document.getElementById('cta');
  var data = null, pro = null, lang = pick(), sizes = {}, listeners = [];
  function mb(n) { return (n / 1048576).toFixed(n < 10485760 ? 1 : 0) + ' MB'; }
  function loadSizes(d) {
    var all = [];
    ['mac', 'win', 'linux', 'android'].forEach(function (k) { (d.files[k] || []).forEach(function (f) { all.push(f); }); });
    return Promise.all(all.map(function (f) {
      if (f.size) { sizes[f.file] = f.size; return null; }
      return fetch(f.file, { method: 'HEAD' }).then(function (r) {
        var n = +r.headers.get('content-length'); if (r.ok && n) sizes[f.file] = n;
      }).catch(function () {});
    }));
  }

  function fileLabel(k, f, t) {
    var ext = (f.file.match(/\.(\w+)$/) || [])[1] || '';
    ext = ext.toLowerCase();
    if (k === 'mac') return [t.dl_mac, t.note_mac];
    if (k === 'win') return [t.dl_win, t.note_win];
    if (ext === 'deb') return ['.deb', t.note_deb];
    if (ext === 'rpm') return ['.rpm', t.note_rpm];
    if (ext === 'appimage') return ['.AppImage', t.note_appimage];
    if (ext === 'apk') return [t.dl_apk || 'APK', t.note_apk || ''];
    return [f.label || f.file, f.note || ''];
  }

  function renderDownloads() {
    if (!data || !HOME) return;
    var t = T[lang];
    ['mac', 'win', 'linux', 'android'].forEach(function (k) {
      var box = document.querySelector('[data-os="' + k + '"] .dl');
      if (!box) return;
      box.innerHTML = '';
      (data.files[k] || []).forEach(function (f) {
        var l = fileLabel(k, f, t);
        var a = document.createElement('a');
        a.href = DL + f.file; a.setAttribute('download', '');
        var s = document.createElement('span'); s.textContent = l[0];
        var n = document.createElement('small'); n.textContent = l[1];
        if (sizes[f.file]) { var b = document.createElement('b'); b.textContent = mb(sizes[f.file]); n.appendChild(b); }
        a.appendChild(s); a.appendChild(n); box.appendChild(a);
      });
    });
    // Installer sizes in the numbers band: the first file of each platform.
    document.querySelectorAll('[data-mb]').forEach(function (el) {
      var f = (data.files[el.getAttribute('data-mb')] || [])[0];
      if (f && sizes[f.file]) el.textContent = mb(sizes[f.file]).replace(' MB', '');
    });
    var apk = (data.files.android || [])[0];
    document.getElementById('meta').textContent = t.version + ' ' + data.version + ' · ' + data.date + ' · macOS · Windows · Linux' + (apk ? ' · Android' : '');
    // The Android card in the download grid, and a one-link line under the
    // main button (on Android the main button is the APK already).
    document.documentElement.classList.toggle('has-apk', !!apk);
    var card = document.querySelector('[data-os="android"]');
    if (card) card.hidden = !apk;
    var an = document.getElementById('andr');
    if (an && apk && !/Android/.test(ua)) {
      an.href = DL + apk.file;
      an.hidden = false;
    }
    document.getElementById('ver').textContent = 'v' + data.version;
    var c = document.getElementById('cta');
    if (/Android/.test(ua) && apk) {
      c.href = DL + apk.file; c.setAttribute('download', '');
      c.textContent = t.cta_os.replace('{os}', 'Android');
      if (card) card.classList.add('hl');
    } else if (os && data.files[os] && data.files[os][0]) {
      c.href = DL + data.files[os][0].file; c.setAttribute('download', '');
      c.textContent = t.cta_os.replace('{os}', OS_NAMES[os]);
      document.querySelector('[data-os="' + os + '"]').classList.add('hl');
    }
    document.dispatchEvent(new Event('downloads-rendered'));
  }

  // The Pro page lists its own files (downloads-pro.json); until they are
  // published it says they are coming.
  function renderPro() {
    var sec = document.getElementById('pro-dl');
    if (!sec || !pro || !pro.files) return;
    var t = T[lang];
    ['mac', 'win', 'linux'].forEach(function (k) {
      var box = sec.querySelector('[data-pro="' + k + '"] .dl');
      box.innerHTML = '';
      (pro.files[k] || []).forEach(function (f) {
        var l = fileLabel(k, f, t);
        var a = document.createElement('a');
        a.href = BASE + DL + f.file; a.setAttribute('download', '');
        var s = document.createElement('span'); s.textContent = l[0];
        var n = document.createElement('small'); n.textContent = l[1];
        if (f.size) { var b = document.createElement('b'); b.textContent = mb(f.size); n.appendChild(b); }
        a.appendChild(s); a.appendChild(n); box.appendChild(a);
      });
    });
    document.getElementById('pro-soon').hidden = true;
    document.getElementById('pro-meta').textContent = t.version + ' ' + pro.version + ' · ' + pro.date;
    var c = document.getElementById('pro-cta');
    if (os && pro.files[os] && pro.files[os][0]) {
      c.href = BASE + DL + pro.files[os][0].file; c.setAttribute('download', '');
      c.textContent = t.cta_os.replace('{os}', OS_NAMES[os]);
      sec.querySelector('[data-pro="' + os + '"]').classList.add('hl');
    }
  }

  function apply(code) {
    lang = code;
    var t = T[code];
    document.documentElement.lang = HTML_LANG[code];
    var page = document.documentElement.getAttribute('data-page');
    document.title = page === 'plugins' ? t.pl_title : page === 'pro' ? t.pro_title : t.title;
    var md = document.querySelector('meta[name="description"]'); if (md) md.content = page === 'plugins' ? t.pl_desc : page === 'pro' ? t.pro_desc : t.desc;
    document.querySelectorAll('[data-i]').forEach(function (el) {
      var key = el.getAttribute('data-i');
      var v = t[key];
      if (v == null && code !== 'zh') v = T.en[key];
      if (v != null) el.innerHTML = v;
    });
    document.querySelectorAll('[data-i-alt]').forEach(function (el) {
      var key = el.getAttribute('data-i-alt');
      var v = t[key];
      if (v == null && code !== 'zh') v = T.en[key];
      if (v != null) el.setAttribute(el.hasAttribute('data-i-aria') ? 'aria-label' : 'alt', v);
    });
    var btn = document.querySelector('#lang > button');
    btn.querySelector('span').textContent = NAMES[code];
    document.querySelectorAll('#lang li button').forEach(function (b) {
      b.setAttribute('aria-current', b.dataset.lang === code ? 'true' : 'false');
    });
    renderDownloads();
    renderPro();
    listeners.forEach(function (fn) { fn(code); });
  }

  // language menu
  var box = document.getElementById('lang');
  var ul = box.querySelector('ul');
  ul.innerHTML = LANGS.map(function (c) {
    return '<li><button type="button" role="menuitem" data-lang="' + c + '" lang="' + HTML_LANG[c] + '">' + NAMES[c] + '</button></li>';
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
  if (document.getElementById('pro-dl')) fetch(BASE + 'downloads-pro.json', { cache: 'no-cache' }).then(function (r) { return r.ok ? r.json() : null; })
    .then(function (d) { pro = d; renderPro(); }).catch(function () {});
})();
