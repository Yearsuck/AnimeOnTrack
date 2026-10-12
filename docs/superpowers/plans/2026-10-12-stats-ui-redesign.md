# Plan: Rediseño y Auditoría UI/UX de Estadísticas

Fecha: 2026-10-12
Branch: `feature/stats-ui-redesign`
Inspirado por:
- `anthropics/skills/frontend-design`: Jerarquía visual intencionada, eliminación de patrones repetitivos tipo "AI-slop/tarjeta SaaS", protagonismo visual equilibrado.
- `vercel-labs/agent-skills/web-design-guidelines`: 100+ reglas de interfaz, accesibilidad (WCAG 2.1 AA), estados de foco y ergonomía.
- `vercel-labs/agent-skills/react-best-practices`: Eliminación de cascadas IPC/fetch, renderizado eficiente y memoización.
- `vercel-labs/agent-skills/composition-patterns`: Arquitectura de componentes limpia y consolidación de controles.

---

## 1. Problemas Identificados y Objetivos

### A. Fallos Visuales y CSS
1. **Bug en `.genrecard` (Desktop)**: Falta `display: flex; flex-direction: row;` en la regla desktop (>600px). Actualmente `.genrecard-rank` (48px) y `.genrecard-body` se comportan como bloques apilados verticalmente.
2. **Affordance engañosa en `Popularity Bias`**: Simula un control deslizante (`.popbias-track` + `.popbias-thumb`), pero no es interactivo. Se reemplazará por un medidor/espectro visual analítico ("Índice de Popularidad").
3. **Contraste insuficiente en `StatsHeatmap`**: El nivel 1 de actividad (`.heatmap-cell.level-1`) usa `opacity: 0.15`, incumpliendo el ratio de contraste WCAG 2.1 AA.

### B. Jerarquía e Información Arquitectónica
1. **Saturación de tarjetas numéricas**: 11 tarjetas en dos rejillas desconectadas (`Stats.tsx` y `StatsInsights.tsx`). Se agruparán en secciones conceptuales con sentido:
   - *Actividad & Tiempo*: Tiempo total, episodios vistos, ritmo de finalización.
   - *Biblioteca & Progreso*: Progreso general, series seguidas, pendientes y lista de deseos.
   - *Hitos*: Récord de maratón, media episodios/serie.
2. **El Grafo 3D enterrado**: El grafo 3D Three.js se encuentra al final de la página. Se reorganizará la cabecera/pestañas de Estadísticas para que el usuario pueda alternar fluidamente entre:
   - **Vista Analítica ("Resumen & Hábitos")**: Métricas, rankings, estudios, épocas, mapa de calor.
   - **Vista Exploratoria ("Grafo 3D & Catálogo")**: El grafo 3D espacial o las tarjetas de géneros/formatos.
3. **Duplicidad de gráficos temporales**: Coexisten el gráfico de 30 días (`DayActivityChart`) y el mapa de calor anual (`StatsHeatmap`). Se unificarán bajo un selector de rango dentro de la sección de Actividad ("Últimos 30 días" / "Año completo").
4. **Duplicidad de `ShapeToggle`**: Se eliminará el doble toggle barras/círculos en favor de una única configuración contextual.

### C. Rendimiento y React
1. **Eliminar cascada de llamadas IPC en `StatsInsights`**: En lugar de 4 hooks `useEffect` independientes con 4 re-renders en cascada, agrupar las consultas complementarias en una sola carga coordinada.
2. **Memoizar transformaciones de datos**: `useMemo` para listas transformadas (`topSeriesData`, `funnelData`, etc.).

---

## 2. Fases de Implementación

### Fase 1: Correcciones de CSS y Diseño Visual
- Corregir `.genrecard` (desktop `display: flex; flex-direction: row; align-items: stretch;`, `.genrecard-body` `flex: 1`).
- Rediseñar `.popbias` como medidor visual con escala (Nicho vs Mainstream) sin falso thumb arrastrable.
- Aumentar opacidad/contraste en `.heatmap-cell.level-1` a un mínimo accesible (0.3).

### Fase 2: Reorganización del Layout y Pestañas
- En `Stats.tsx`, elevar las pestañas principales al encabezado:
  - `Resumen & Hábitos` (métricas, distribución, actividad)
  - `Grafo Galáctico 3D` (Three.js con pantalla completa/inmersiva)
  - `Distribución por Géneros` (tarjetas de géneros y tipos)
- Organizar las métricas clave en una rejilla clara y sin repeticiones.

### Fase 3: Optimización de Rendimiento y React
- Unificar las llamadas de `StatsInsights` en una sola promesa coordinada.
- Aplicar `useMemo` a los conjuntos de datos derivados.
- Añadir accesibilidad de teclado (`tabIndex`, aria-label) a las barras del gráfico diario y horario.

---

## 3. Criterios de Aceptación y Verificación
- `npx tsc --noEmit` compila sin errores de tipos.
- `npm run build` genera el bundle de producción sin incidencias.
- Las tarjetas de géneros se visualizan en fila horizontal en pantallas de escritorio y responsive en móvil.
- El grafo 3D es inmediatamente accesible desde las pestañas principales sin necesidad de scroll vertical infinito.
- No hay re-renders innecesarios al cargar Estadísticas.
