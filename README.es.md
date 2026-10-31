[English](README.md) | [简体中文](README.zh-CN.md) | [Deutsch](README.de.md) | [日本語](README.ja.md) | **Español** | [Français](README.fr.md) | [Русский](README.ru.md)

# TifMrcView

Un visor pequeño, rápido y cómodo para imágenes de microscopía y microscopía electrónica: TIFF / OME-TIFF / ImageJ, MRC para crio-EM y crio-ET, Zeiss CZI y SmartSEM. Dibuje líneas, rectángulos y círculos con mediciones en unidades físicas, cambie entre las vistas XY / XZ / YZ y vea fluorescencia multicanal con activación y color por canal. El instalador ocupa solo unos pocos MB y aun así abre pilas de varios GB al instante. Funciona en macOS, Windows y Linux, y se puede ampliar con plugins. Creado con Rust + Tauri 2.

[![Download](https://img.shields.io/badge/download-tifmrcview.com-2f6fed)](https://tifmrcview.com/tifmrcview/#download)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey)
[![GitHub stars](https://img.shields.io/github/stars/Leehow/TifMrcView?style=social)](https://github.com/Leehow/TifMrcView)

**[Descargar](https://tifmrcview.com/tifmrcview/#download)** · **[Mercado de plugins](https://tifmrcview.com/tifmrcview/plugins/)** · [Sitio web](https://tifmrcview.com/tifmrcview/)

Autor: **Hao Li** · [ORCID](https://orcid.org/0009-0004-2696-3267) · [@Leehow](https://github.com/Leehow) · [lihao9x@iccas.ac.cn](mailto:lihao9x@iccas.ac.cn)

> Si TifMrcView le resulta útil, dele una ⭐ en GitHub: ayuda de verdad.

## Capturas de pantalla

<table>
  <tr>
    <td width="50%"><a href="docs/shots/mrc-tomogram-line.webp"><img src="docs/shots/mrc-tomogram-line-1200.webp" alt="Tomograma crio-ET en MRC con medición de longitud"></a><br><sub>Tomograma crio-ET (MRC) con medición de longitud y las herramientas de anotación.</sub></td>
    <td width="50%"><a href="docs/shots/mrc-xz-view-box.webp"><img src="docs/shots/mrc-xz-view-box-1200.webp" alt="Vista lateral XZ con un rectángulo"></a><br><sub>Vista lateral XZ con un rectángulo que muestra ancho × alto.</sub></td>
  </tr>
  <tr>
    <td width="50%"><a href="docs/shots/czi-airyscan-measure.webp"><img src="docs/shots/czi-airyscan-measure-1200.webp" alt="Pila Z Airyscan de Zeiss CZI con medición"></a><br><sub>Pila Z Airyscan de Zeiss CZI con medición y barra de escala.</sub></td>
    <td width="50%"><a href="docs/shots/tiff-multichannel.webp"><img src="docs/shots/tiff-multichannel-1200.webp" alt="TIFF de fluorescencia de 3 canales"></a><br><sub>TIFF de fluorescencia de 3 canales con activación por canal.</sub></td>
  </tr>
</table>

## Características

- Formatos compatibles: TIFF (multipágina, bandas/teselas, compresión LZW/Deflate/PackBits/JPEG, escala de grises de 1–7 bits como binario de 1 bit y 4 bits, y enteros y coma flotante de 8/16/32 bits; las profundidades poco habituales, como 12 bits, dan un mensaje de error claro), OME-TIFF (lee las dimensiones C/Z/T, nombres y colores de canal, tamaño de píxel), hiperpilas de ImageJ (incluidos archivos contiguos de más de 4 GB), TIFF con paleta (como las imágenes SEM de Zeiss), MRC/MRC2014 (modos 0/1/2/3/4/6/12/101, `.mrc` `.mrcs` `.rec` `.st` `.map`; en los archivos de modo 0 sin marca IMOD se decide si son bytes sin signo según el mínimo/máximo de la cabecera), montajes de SerialEM (mapas de rejilla y atlas; se ensamblan con las coordenadas de las piezas del `.mdoc` o de la cabecera extendida, y las piezas individuales siguen disponibles como segunda imagen) y, con el plugin Zeiss, Zeiss CZI (LSM / Airyscan, etc. guardados por ZEN: subbloques sin comprimir, Zstd y JPEG, escala de grises y color de 8/16/32 bits, dimensiones C/Z/T, mosaicos en teselas, nombres y colores de canal, tamaño de píxel). `.rec` `.st` `.map` y `.czi` solo aparecen en «Abrir con» y no se establecen como aplicación predeterminada
- También abre PNG / JPEG / BMP / GIF / WebP
- Control deslizante Z y reproducción; con dimensión temporal, también un control T
- Microscopía electrónica de volumen (FIB-SEM, cortes seriados, SBF-SEM): una carpeta de imágenes de cortes se abre como una pila Z (arrastra la carpeta a la ventana, o el botón de carpeta / Ctrl/⌘+Mayús+O); los cortes pueden estar en cualquier formato que abra la aplicación y solo se leen cuando hacen falta. Cuando un plugin reconoce un archivo de proyecto de adquisición cercano (`ProjectData.dat` de Thermo Fisher Auto Slice & View con el plugin `thermo`), la lista de miniaturas de esa carpeta añade una entrada de pila con el grosor de corte como espaciado Z; si no, el espaciado Z se puede escribir en «Barra de escala»
- Composición de color multicanal: la casilla muestra/oculta un canal, pulsar el nombre ajusta el contraste de ese canal y pulsar el punto cambia su color
- Brillo / contraste / gamma; arrastre sobre el histograma para elegir directamente el rango visible; contraste automático por corte de forma predeterminada (saturación del 0,1 %)
- El botón ⓘ de la barra de herramientas muestra la información de la imagen: formato, tamaño, tamaño de píxel y parámetros de adquisición leídos por plugins (con búsqueda y copia). El plugin Zeiss lee todos los parámetros de una imagen SEM de Zeiss SmartSEM (voltaje, distancia de trabajo, aumento, detector, posición de la platina, vacío, …) y muestra una barra de escala según el tamaño de píxel calibrado, y de un CZI el microscopio, objetivo, láseres, detectores, pinhole, escaneo y parámetros de procesamiento Airyscan
- Una barra de vista previa a la derecha (plegable) lista las imágenes de la misma carpeta con nombre, formato y tamaño
- Barra de iconos superior (nombres al pasar el ratón): guardar imagen, deshacer, seleccionar, mover, medir, lápiz (la flecha de al lado abre un menú: libre / línea / flecha / rectángulo / círculo), texto, recortar, barra de escala, brillo/contraste
- Las mediciones muestran longitudes reales según el tamaño de píxel; los rectángulos y círculos muestran siempre ancho × alto sobre la imagen (el diámetro en un círculo perfecto)
- Herramienta de selección (predeterminada): arrastre un espacio vacío para desplazar la imagen; pulse una anotación para arrastrarla, arrastre los puntos del borde para cambiar su tamaño, arrastre los extremos de una línea de medición para volver a medir; Supr elimina; se pueden cambiar el color y el grosor de línea
- Las anotaciones se guardan por corte Z y se almacenan automáticamente unos 2 segundos después de dibujar en `<nombre del archivo>.annot.json` junto a la imagen, y se vuelven a leer la próxima vez que se abre
- Recorte: enmarque una región y haga doble clic o pulse Intro para ver solo esa zona; Esc o Retroceso lo deshace
- Un modelo IMOD (`.mod` / `.fid`) con el mismo nombre se superpone automáticamente y se puede activar por objeto
- Guardar imagen: PNG a la resolución original (tras recortar, solo el área recortada), con las anotaciones, el modelo y la barra de escala visibles
- Las imágenes con Z pueden alternar arriba entre las vistas XY / XZ / YZ (las anotaciones dibujadas en las vistas laterales no se guardan)
- Arrastre el borde interior de la barra de herramientas izquierda y de la barra de vista previa derecha para cambiar su ancho
- Rueda del ratón para ampliar, arrastrar para desplazar, doble clic para ajustar a la ventana; la barra de estado muestra las coordenadas del píxel y el valor original
- Arrastre un archivo a la ventana para abrirlo; hacer doble clic en otro archivo de la misma carpeta lo abre en la ventana existente, y un archivo de otra carpeta abre una ventana nueva
- El botón «Abrir carpeta» de la parte superior de la barra de vista previa muestra la imagen actual en el Finder / Explorador
- Con cualquier otra herramienta activa, el clic derecho sobre la imagen vuelve a la herramienta de selección
- Una anotación puede mostrarse en un solo corte o abarcar un rango de Z; tras seleccionarla se cambia su Z arriba, y el control Z marca los cortes que tienen anotaciones

## Plugins

En «Plugins», en la barra de herramientas, se pueden activar/desactivar, instalar y desinstalar plugins y buscar actualizaciones. Salvo los idiomas, cada plugin se instala por separado: pulse «Mercado de plugins» para abrir https://tifmrcview.com/tifmrcview/plugins/ en el navegador y descargar un paquete de plugin (.zip); después pulse «Cargar plugin…» y selecciónelo. Instalar una versión más reciente del mismo plugin reemplaza simplemente a la anterior. El código fuente de los plugins está en el repositorio, en `plugins/<id>/` (`plugin.json` + `index.js`; actualmente `recorder`, `zeiss`, `lasx`, `nis`, `fluoview`, `imaris`, `gatan`, `thermo` y `tvips`); los plugins instalados están en una subcarpeta `<id>/` de la carpeta de plugins.

Instalación con un clic (0.5 o posterior): pulse **Instalar** en la tarjeta de un plugin en el mercado, o en la lista «Mercado de plugins» del menú Plugins (que muestra «Actualizar a x.y.z» cuando hay una versión nueva). La web pasa a la aplicación un enlace del tipo `tifmrcview://install-plugin?id=<id>` (opcionalmente `&version=<x.y.z>`); la aplicación muestra el nombre y la descripción del plugin y no instala nada hasta que pulse **Instalar**. Solo instala plugins publicados en el mercado oficial (`plugins.json` junto al canal de actualizaciones), los descarga por HTTPS y comprueba el tamaño y el SHA-256 antes de instalarlos. Descargar el zip y usar «Instalar plugin…» sigue funcionando.

Plugins de formato: un paquete de plugin también puede añadir formatos de archivo y lectores de metadatos como un módulo WebAssembly aislado, escrito por ejemplo en Rust con el SDK de `plugins/sdk`. Cada fabricante tiene un plugin así (`zeiss` abre CZI y LSM y lee los parámetros de SmartSEM), de modo que el programa no contiene código de formatos de fabricantes: sin el plugin Zeiss, los .czi no se abren y la aplicación indica qué plugin instalar. Un módulo solo ve el archivo que se abre, funciona con límites de memoria y tiempo y describe cada página como una lista de fragmentos del archivo que la aplicación descomprime y monta en paralelo. La ABI, los límites y un ejemplo están en [docs/format-plugins.md](docs/format-plugins.md).

Plugin de grabación de vídeo: en el panel «Herramientas», en «Grabar vídeo (MP4)», elija Z o T, el fotograma inicial y final, la tasa de fotogramas, la región (imagen completa / selección / vista actual), el tamaño y la calidad. El MP4 H.264 resultante incluye el contraste actual, los colores de canal, las anotaciones y la barra de escala.

El idioma de la interfaz también es un plugin integrado (`ui/plugins/i18n.js`): admite 中文, English, Deutsch, 日本語, Español, Français y Русский, sigue el idioma del sistema de forma predeterminada y se puede cambiar con el menú desplegable de la barra de herramientas. Sustituye los textos chinos conocidos de la interfaz por el idioma elegido, de modo que también se traducen otros plugins y mensajes; para añadir texto nuevo, añada una fila a `WORDS` (texto fijo) o `PATTERNS` (texto con números o nombres de archivo). Al desactivar este plugin se vuelve al chino.

Escribir su propio plugin: ponga un archivo `.js` en la carpeta de plugins y pulse «Recargar». Para distribuirlo, empaquete `plugin.json` (`id`, `name`, `version`, `description`, `main`, `apiVersion`, `minAppVersion`, y opcionalmente `author`, `i18n`, `details`; vea el formato en `plugins/recorder/plugin.json`) junto con el script en un zip (archivos en la raíz del zip). Un ejemplo mínimo:

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

`app` ofrece: `state()` (archivo actual, Z/T, tamaño, selección, tamaño de píxel), `on("open" | "slice" | "render", fn)`, `setZ` / `setT` / `open(path)`, `renderRegion(region, {z, t, size})` (renderiza en un canvas con los ajustes de visualización actuales, incluidas anotaciones y barra de escala), `ui.addToolSection` / `ui.addToolbarButton` / `ui.addToolbarSelect` / `ui.toast` / `ui.busy`, `dialog.save`, `fs.write`, `settings.get/set`, y `native(method, args, bytes)` para llamar al lado Rust. La definición completa está en `ui/plugin-host.js`. Los plugins que necesitan código nativo (como el codificador H.264 de la grabación) implementan `NativePlugin` en `src-tauri/src/plugins/`; la parte nativa se compila en el programa principal, pero solo se activa cuando se instala un paquete de plugin con el mismo id. Una característica de Cargo controla si la grabación se compila (`recorder` está activada de forma predeterminada; `--no-default-features` la quita y reduce el instalador en aproximadamente 1 MB).

Los plugins tienen los mismos permisos que el programa principal: instale solo plugins de confianza.

## Actualizaciones automáticas

Unos segundos después de iniciarse, la aplicación comprueba en segundo plano `https://tifmrcview.com/tifmrcview/latest.json`. Cuando hay una versión nueva solo aparece un pequeño punto rojo en el engranaje (ajustes), sin avisos ni ventanas. Al abrir el menú del engranaje se ve «Versión x.y.z disponible» y el punto desaparece para esa versión; esa opción abre «Complementos y actualizaciones», donde «Actualizar a x.y.z» la descarga, se verifica la firma, se instala y se reinicia. Los paquetes de actualización se firman con una clave minisign de Tauri; la clave pública está en `tauri.conf.json`.

## Atajos de teclado

| Tecla | Acción |
| --- | --- |
| ↑ / ↓ | Archivo anterior / siguiente |
| ← / →, RePág / AvPág, `,` / `.` | Cambiar Z |
| Inicio / Fin | Primer / último corte |
| Espacio | Reproducir / pausar Z |
| Ctrl + rueda (⌘ + rueda en macOS), Mayús + rueda | Cambiar Z |
| S / V / M / P / L / A / R / E / X / C | Seleccionar / mover / medir / lápiz / línea / flecha / rectángulo / círculo / texto / recortar |
| Mayús (al dibujar un rectángulo o círculo) | Cuadrado / círculo perfecto |
| Mayús (al dibujar una línea o flecha) | Ajustar a 45° |
| Supr / Retroceso | Eliminar la anotación seleccionada; sin selección, deshacer el recorte |
| Esc | Cancelar la anotación que se está dibujando o recién dibujada, deseleccionar, deshacer el recorte |
| Intro, doble clic en el marco de recorte | Recortar |
| Ctrl/⌘ + Z | Deshacer (se pueden deshacer dibujar, medir, mover, eliminar y recortar) |
| Ctrl/⌘ + Mayús + Z, Ctrl/⌘ + Y | Rehacer |
| Ctrl/⌘ + S | Guardar un PNG (junto a la imagen de forma predeterminada, o en la carpeta de guardado elegida en el panel Herramientas) |
| H | Mostrar / ocultar anotaciones |
| I | Mostrar / ocultar el modelo IMOD |
| T | Abrir / plegar el panel «Herramientas» |
| B | Mostrar / ocultar la barra de vista previa |
| F, doble clic (herramienta mover) | Ajustar a la ventana |
| 1 | Tamaño original (1:1) |
| + / =, - | Acercar / alejar |
| Ctrl/⌘ + O | Abrir archivo |

## Por qué es rápido

- Los archivos se leen mediante mapeo de memoria y las bandas/teselas de una página se descomprimen en paralelo en todos los núcleos de la CPU
- Tras abrir un archivo, el resto de cortes Z se decodifican previamente en memoria en segundo plano, empezando por los más cercanos al actual (con aproximadamente una cuarta parte de la memoria física, 512 MB–8 GB), de modo que al arrastrar el control se muestran las imágenes al instante
- Solo se renderiza la región visible y solo a la resolución de la pantalla; el RGBA de 8 bits se pasa a la interfaz en binario, por lo que ampliar y desplazar una imagen de 4000×4000 sigue siendo fluido
- Las miniaturas se generan en un grupo de hilos aparte para no competir por la CPU con la imagen actual
- La interfaz es HTML/JS puro, sin framework de frontend; las compilaciones de release usan LTO y strip, y los instaladores ocupan unos 4–11 MB (el AppImage de Linux incluye WebKit, por lo que ocupa unos 84 MB)

## Instalación

Descargue desde el sitio oficial **[tifmrcview.com/tifmrcview](https://tifmrcview.com/tifmrcview/#download)**. Las versiones nuevas salen a menudo y solo se publican allí (no se usan GitHub Releases); la aplicación también se actualiza desde ese sitio:

- **macOS** (10.15 o posterior): `TifMrcView_x.y.z_universal.dmg` (Intel y Apple silicon); arrástrelo a «Aplicaciones». Como la aplicación no tiene firma de desarrollador de Apple, el primer inicio se bloquea: vaya a «Ajustes del Sistema → Privacidad y seguridad» y pulse «Abrir igualmente», o ejecute `xattr -dr com.apple.quarantine /Applications/TifMrcView.app` en una terminal. En el primer inicio, TifMrcView se establece como aplicación predeterminada para .tif/.tiff/.mrc.
- **Windows**: `TifMrcView_x.y.z_x64-setup.exe`; haga doble clic para instalar (no requiere permisos de administrador). El instalador registra las asociaciones de .tif/.tiff/.mrc; si antes eligió manualmente otro programa para .tif (por ejemplo «Fotos»), Windows no permite que el software lo cambie en silencio: pulse «Establecer como predeterminada» en TifMrcView, que abre la página de ajustes del sistema, y elija allí TifMrcView.
- **Linux**: `.deb` para Debian/Ubuntu, `.rpm` para Fedora y similares, `.AppImage` para otras distribuciones. En el primer inicio se establece como aplicación predeterminada mediante `xdg-mime`.

## Compilar desde el código fuente

Se necesita Rust (stable) y Node.js 18+. En Linux, además:

```sh
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libxdo-dev libssl-dev
```

Después:

```sh
npm install
npm run dev      # modo de desarrollo
npm run build    # genera el instalador del sistema actual, en src-tauri/target/release/bundle/
```

Para depurar la decodificación hay una pequeña herramienta incluida; imprime metadatos, píxeles por canal y tiempos de decodificación/renderizado:

```sh
cd src-tauri && cargo run --release --example probe -- /path/to/file.tif
```

## Publicación de versiones

Los números de versión se generan automáticamente: el mayor.menor de `tauri.conf.json` (actualmente 0.3) más el número total de commits, por ejemplo `0.3.57`; al subir una etiqueta `v1.2.3` se usa la versión de la etiqueta.

Cada push a `main` hace que GitHub Actions compile paquetes en macOS / Windows / Linux; los instaladores están en los Artifacts de la ejecución. Hay dos formas de publicar en el sitio de descargas:

- Subir una etiqueta que empiece por `v`: `git tag v0.3.0 && git push origin v0.3.0` sube al sitio de descargas y crea un GitHub Release;
- o ejecutar manualmente el build en Actions marcando «publicar en el sitio de descargas», lo que publica con el número de versión automático.

Al publicar, la CI coloca los instaladores y los paquetes de actualización en el directorio `<versión>/` del sitio de descargas y, por último, actualiza `latest.json` (para la actualización automática) y `downloads.json` (para la página de descargas). Los paquetes del mercado de plugins (`plugins/`) también se despliegan con su propio workflow (`.github/workflows/plugins.yml`) cuando cambian en `main`; las páginas del sitio web están en un repositorio aparte. Configure lo siguiente en el repositorio, en Settings → Secrets and variables → Actions:

| Nombre | Tipo | Descripción |
| --- | --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | Secret | Clave privada para firmar los paquetes de actualización (si se pierde, ya no se pueden enviar actualizaciones a los usuarios que lo tienen instalado) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Secret, opcional | Contraseña de esa clave, si la tiene |
| `DEPLOY_SSH_KEY` | Secret | Clave privada SSH del usuario de despliegue del sitio de descargas |
| `DEPLOY_HOST` / `DEPLOY_USER` | Secret | Host y nombre de usuario del sitio de descargas |
| `UPDATE_BASE_URL` | Variable, opcional | Por defecto `https://tifmrcview.com/tifmrcview` |
| `DEPLOY_PATH` | Variable, opcional | Por defecto `/var/www/tifmrcview` |

## Estructura del proyecto

| Ruta | Contenido |
| --- | --- |
| `src-tauri/src/formats/` | Decodificadores: `tiff.rs`, `mrc.rs`, `raster.rs` (PNG/JPEG/…), `mod.rs` |
| `src-tauri/src/` | `lib.rs` (comandos, gestión de ventanas y archivos), `render.rs`, `reslice.rs` (vistas XZ/YZ), `image.rs`, `imod.rs` (modelos IMOD), `mem.rs`, `assoc.rs` (registro como aplicación predeterminada), `update.rs` |
| `src-tauri/src/plugins/` | Partes nativas de los plugins (`recorder.rs`, `mp4.rs`), paquetes de plugins (`package.rs`) y el host WebAssembly de los plugins de formato (`wasm.rs`, `recipe.rs`, `registry.rs`) |
| `src-tauri/examples/probe.rs` | La herramienta de sondeo de decodificación |
| `ui/` | Frontend en HTML/JS puro: `index.html`, `app.js`, `style.css`, `plugin-host.js`; plugin de idioma integrado en `ui/plugins/i18n.js` |
| `plugins/<id>/` | Código fuente de los plugins instalables (`recorder` y los plugins de formato `zeiss`, `lasx`, `nis`, `fluoview`, `imaris`, `gatan`, `thermo` y `tvips`, uno por fabricante, crates de Rust compilados a WebAssembly) y el SDK de plugins (`sdk`) |
| `scripts/` | `pack-plugins.mjs` (genera los zip de plugins y el índice del mercado), `make-release.mjs` |
| `docs/shots/` | Capturas usadas en este README |
| `packaging/linux/` | Archivos de empaquetado para Linux (tipo MIME de .mrc) |
| `.github/workflows/` | `build.yml` (compilación y publicación), `plugins.yml` (paquetes del mercado de plugins) |

## Traducciones

El README está disponible en varios idiomas (vea la línea de idiomas al principio). **El README en inglés (`README.md`) es la fuente de referencia**; los demás archivos mantienen exactamente las mismas secciones en el mismo orden. Cuando cambie `README.md`, actualice también las traducciones o abra un issue si no puede.

## Autor

<!-- author -->
Creado y mantenido por **Hao Li** ([@Leehow](https://github.com/Leehow)), Institute of Chemistry, Chinese Academy of Sciences. ORCID: [0009-0004-2696-3267](https://orcid.org/0009-0004-2696-3267)
<!-- /author -->

## Cómo citar

Si TifMrcView le resulta útil en su investigación, cítelo. GitHub muestra la cita de [CITATION.cff](CITATION.cff) en «Cite this repository» en la página del repositorio.

## Licencia

Este proyecto es de código abierto bajo la [Apache License 2.0](LICENSE).
