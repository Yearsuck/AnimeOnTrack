<div align="center">

<img src="https://capsule-render.vercel.app/api?type=waving&color=0:0b1521,100:4aa8ff&height=220&section=header&text=AnimeOnTrack&fontSize=58&fontColor=e9eff5&fontAlignY=38&animation=fadeIn&desc=Deja%20de%20refrescar%20la%20web.%20Que%20refresque%20la%20app.&descAlignY=58&descSize=17&descColor=9fd4ff" width="100%" alt="AnimeOnTrack" />

<a href="https://github.com/Yearsuck/AnimeOnTrack">
  <img src="https://readme-typing-svg.demolab.com?font=Fira+Code&weight=500&size=17&pause=1400&color=4AA8FF&center=true&vCenter=true&width=560&lines=Sigue+lo+que+est%C3%A1+en+emisi%C3%B3n...;Cloudflare+no+nos+detiene+%F0%9F%98%A4;Cero+episodios+perdidos%2C+cero+huecos;Windows+%C2%B7+Tauri+%C2%B7+Rust+%C2%B7+React" alt="typing banner" />
</a>

<br/>

<img src="https://img.shields.io/badge/Tauri-2-4aa8ff?style=for-the-badge&logo=tauri&logoColor=e9eff5&labelColor=0b1521" alt="Tauri" />
<img src="https://img.shields.io/badge/Rust-stable-4aa8ff?style=for-the-badge&logo=rust&logoColor=e9eff5&labelColor=0b1521" alt="Rust" />
<img src="https://img.shields.io/badge/React-18-4aa8ff?style=for-the-badge&logo=react&logoColor=e9eff5&labelColor=0b1521" alt="React" />
<img src="https://img.shields.io/badge/TypeScript-strict-4aa8ff?style=for-the-badge&logo=typescript&logoColor=e9eff5&labelColor=0b1521" alt="TypeScript" />
<img src="https://img.shields.io/badge/SQLite-embedded-46d19e?style=for-the-badge&logo=sqlite&logoColor=0b1521&labelColor=0b1521" alt="SQLite" />
<img src="https://img.shields.io/badge/platform-Windows-e9eff5?style=for-the-badge&logo=windows11&logoColor=0b1521&labelColor=0b1521" alt="Windows" />

</div>

<br/>

## La premisa

Hay un sitio pirata con casi todo lo que está en emisión. Está detrás de Cloudflare, así que ningún cliente HTTP normal pasa del `403` — hace falta un navegador de verdad resolviendo el reto JS. **AnimeOnTrack** abre una ventana WebView2 oculta, espera a que el reto caiga, y te avisa cuándo hay episodio nuevo de lo que sigues. Tú decides qué ver; la app decide cuándo hay que refrescar.

<div align="center">
<img src="https://capsule-render.vercel.app/api?type=soft&color=0:4aa8ff,100:0b1521&height=3&section=header&reversal=false" width="100%" alt="" />
</div>

## Por dentro de la app

La navegación son 4 pestañas hoy, 6 en cuanto aterricen los specs de `docs/superpowers/specs/`:

| Pestaña | Qué hace |
|---|---|
| **Pendientes** | Cola plana de episodios nuevos de todo lo que sigues, más recientes primero. |
| **En emisión** | Catálogo scrapeado de lo que está emitiéndose esta temporada — sigue lo que te interese con un clic. |
| **Biblioteca** | Tus series seguidas con barra de progreso, marcado gap-free episodio a episodio. |
| **Ajustes** | Mirrors del sitio (el original cae, los espejos aguantan) y fuente configurable. |
| 🔜 **Descubrir** | Modo swipe: te enseña finalizados al azar, decides *ya lo vi* / *quiero ver* / *paso*. |
| 🔜 **Stats** | Cuántos animes has visto, de qué género, y de qué tipo — sacado de lo que marques en Descubrir. |

**Ver un episodio marca todo lo anterior como visto. Desmarcar uno desmarca todo lo posterior.** Nada de huecos tipo "vi el 10 pero no el 6-9" — si le das a un check, la app entiende exactamente lo que quisiste decir, aunque el número del episodio sea raro (`1x05`, `12.5`, lo que sea).

<div align="center">
<img src="https://capsule-render.vercel.app/api?type=soft&color=0:4aa8ff,100:0b1521&height=3&section=header&reversal=false" width="100%" alt="" />
</div>

## Por qué existe cada decisión rara

- **Ventana WebView2 en vez de `reqwest`** — Cloudflare exige un motor JS real; un cliente HTTP con user-agent falso nunca pasa del reto.
- **Portadas de una en una, solo para lo seguido** — pedir ~150 pósters de golpe se lee como abuso y te banea aunque la sesión sea válida. Una portada por serie seguida, por refresco, y punto.
- **Fallback a espejos que no se rinde con un fetch OK** — un mirror puede responder `200` con una web totalmente distinta. Se sigue probando hasta que el parseo trae datos de verdad, no solo hasta que el servidor responde.
- **El scraper de finalizados nunca hace scraping en bloque** *(spec, aún sin construir)* — el sitio no tiene un catálogo único, solo listas por género paginadas. El modo Descubrir pide un lote de 10 a la vez, nunca el catálogo entero.

<div align="center">
<img src="https://capsule-render.vercel.app/api?type=soft&color=0:4aa8ff,100:0b1521&height=3&section=header&reversal=false" width="100%" alt="" />
</div>

## Arrancarlo

```bash
# Backend
cargo build --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml

# Frontend
npx tsc --noEmit
npm run build

# App completa, con hot-reload
npm run tauri dev
```

Requiere `%USERPROFILE%\.cargo\bin` en el `PATH` (toolchain `stable-x86_64-pc-windows-msvc`). La base SQLite vive en `%APPDATA%\com.ernes.aot-scaffold\animeontrack.sqlite` si quieres fisgar el estado directamente.

<div align="center">
<img src="https://capsule-render.vercel.app/api?type=soft&color=0:4aa8ff,100:0b1521&height=3&section=header&reversal=false" width="100%" alt="" />
</div>

## Hoja de ruta

Diseñado y documentado en `docs/superpowers/specs/`, todavía sin construir:

1. **Scraper de finalizados + géneros** — selectores reales ya confirmados en el sitio (`.bsx`, `.status.Completed`, `.genxed`).
2. **Modo Descubrir** — swipe con 3 decisiones, deshacer, listas de "quiero ver" y "descartados".
3. **Stats** — ranking de géneros, desglose por tipo, episodios vistos, tendencia mensual.

<br/>

<div align="center">
<img src="https://capsule-render.vercel.app/api?type=waving&color=0:4aa8ff,100:0b1521&height=140&section=footer&animation=fadeIn" width="100%" alt="" />

<sub>Proyecto personal. No afiliado con ningún sitio de streaming. Para verlo, no para redistribuirlo.</sub>

</div>
