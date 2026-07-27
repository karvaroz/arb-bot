export const LOCALES = ["en", "es", "nl"] as const;
export type Locale = (typeof LOCALES)[number];

export const LOCALE_LABELS: Record<Locale, string> = {
  en: "English",
  es: "Español",
  nl: "Nederlands",
};

// Flat key -> per-locale string. Small, static string set (4 pages) — a
// plain object is simpler than pulling in i18next for this.
const en = {
  "nav.overview": "Overview",
  "nav.pools": "Pools",
  "nav.opportunities": "Opportunities",
  "nav.research": "Research",

  "overview.title": "Overview",
  "overview.apiUp": "API up",
  "overview.apiDown": "API down",
  "overview.wsConnected": "WS connected",
  "overview.wsDisconnected": "WS disconnected",
  "overview.opportunitiesPerTick": "Opportunities/tick",
  "overview.poolUpdatesSec": "Pool updates/sec",
  "overview.scannerMs": "Scanner ms",
  "overview.simulationMs": "Simulation ms",
  "overview.liveEvents": "Live events ({count})",
  "overview.waitingForEvents": "Waiting for events over WS…",

  "pools.title": "Pools ({count})",
  "pools.filterPlaceholder": "Filter by pool address",
  "pools.loading": "Loading…",
  "pools.colDex": "DEX",
  "pools.colTokenA": "Token A",
  "pools.colTokenB": "Token B",
  "pools.colReady": "Ready",
  "pools.ready": "ready",
  "pools.notReady": "loading",
  "pools.detailTitle": "Pool detail",
  "pools.detailId": "Pool address",
  "pools.clickForDetail": "Click a row for full addresses",
  "pools.paginationInfo": "Page {page} of {totalPages}",

  "opportunities.title": "Opportunities ({count})",
  "opportunities.subtitle": "Historical, from SQLite — most recent 500 rows.",
  "opportunities.loading": "Loading…",
  "opportunities.empty":
    "None recorded yet. That's an honest empty state, not a bug — see \"How an opportunity is defined\" below for why most candidate routes don't qualify.",
  "opportunities.colSlot": "Slot",
  "opportunities.colProfit": "Profit (lamports)",
  "opportunities.colMargin": "Margin (bps)",
  "opportunities.colImpact": "Max impact (bps)",
  "opportunities.colFragile": "Fragile",
  "opportunities.fragile": "fragile",
  "opportunities.solid": "solid",
  "opportunities.colRoute": "Route",
  "opportunities.hops": "{count} hops",

  "opportunities.methodology.title": "How an opportunity is defined",
  "opportunities.methodology.notional":
    "Every route is quoted with a fixed 1 SOL notional (hardcoded for now — not per-pool sized).",
  "opportunities.methodology.hops":
    "Routes are 2 to {maxHops} hops (config: scanner.max_hops), starting and ending on a base token (SOL/USDC/USDT).",
  "opportunities.methodology.profit":
    "A route only counts at all if it returns strictly more than it started with (expected_profit > 0).",
  "opportunities.methodology.minProfit":
    "It must also clear a minimum profit margin of {minProfitBps} bps (config: scanner.min_profit) — a route that barely breaks even doesn't count as a real opportunity.",
  "opportunities.methodology.impact":
    "max_price_impact_bps is the worst single-hop price impact across the route, as quoted by each DEX's own pricing model.",
  "opportunities.methodology.fragile":
    "An opportunity is flagged fragile when that impact already consumes the whole margin (max_price_impact_bps ≥ profit_margin_bps) — meaning any real execution delay or size is expected to erase the profit. Fragile opportunities are not filtered out, just labeled.",

  "research.title": "Research",
  "research.totalOpportunities": "Total opportunities",
  "research.profitable": "Profitable",
  "research.profitableNotFragile": "Profitable & not fragile",
  "research.avgProfit": "Avg profit (lamports)",
  "research.chartCaption":
    "Margin vs. price impact — a point above the diagonal means the AMM's own quoted impact already consumes the edge (fragile)",
  "research.axisMargin": "profit margin (bps)",
  "research.axisImpact": "max price impact (bps)",
  "research.sampleNote":
    "Showing {sampleSize} of {total} total — stats above are computed over this sample, not the full history.",

  "language.label": "Language",
  "theme.switchToDark": "Switch to dark mode",
  "theme.switchToLight": "Switch to light mode",
};

const es: typeof en = {
  "nav.overview": "Resumen",
  "nav.pools": "Pools",
  "nav.opportunities": "Oportunidades",
  "nav.research": "Investigación",

  "overview.title": "Resumen",
  "overview.apiUp": "API activa",
  "overview.apiDown": "API caída",
  "overview.wsConnected": "WS conectado",
  "overview.wsDisconnected": "WS desconectado",
  "overview.opportunitiesPerTick": "Oportunidades/tick",
  "overview.poolUpdatesSec": "Actualizaciones de pools/seg",
  "overview.scannerMs": "Scanner ms",
  "overview.simulationMs": "Simulación ms",
  "overview.liveEvents": "Eventos en vivo ({count})",
  "overview.waitingForEvents": "Esperando eventos por WS…",

  "pools.title": "Pools ({count})",
  "pools.filterPlaceholder": "Filtrar por dirección de pool",
  "pools.loading": "Cargando…",
  "pools.colDex": "DEX",
  "pools.colTokenA": "Token A",
  "pools.colTokenB": "Token B",
  "pools.colReady": "Listo",
  "pools.ready": "listo",
  "pools.notReady": "cargando",
  "pools.detailTitle": "Detalle del pool",
  "pools.detailId": "Dirección del pool",
  "pools.clickForDetail": "Hacé click en una fila para ver las direcciones completas",
  "pools.paginationInfo": "Página {page} de {totalPages}",

  "opportunities.title": "Oportunidades ({count})",
  "opportunities.subtitle": "Histórico, desde SQLite — últimas 500 filas.",
  "opportunities.loading": "Cargando…",
  "opportunities.empty":
    "Todavía no hay registros. Es un estado vacío honesto, no un bug — mirá \"Cómo se define una oportunidad\" más abajo para entender por qué la mayoría de las rutas candidatas no califican.",
  "opportunities.colSlot": "Slot",
  "opportunities.colProfit": "Ganancia (lamports)",
  "opportunities.colMargin": "Margen (bps)",
  "opportunities.colImpact": "Impacto máx. (bps)",
  "opportunities.colFragile": "Frágil",
  "opportunities.fragile": "frágil",
  "opportunities.solid": "sólida",
  "opportunities.colRoute": "Ruta",
  "opportunities.hops": "{count} saltos",

  "opportunities.methodology.title": "Cómo se define una oportunidad",
  "opportunities.methodology.notional":
    "Cada ruta se cotiza con un monto fijo de 1 SOL (hardcodeado por ahora — no ajustado por pool).",
  "opportunities.methodology.hops":
    "Las rutas van de 2 a {maxHops} saltos (config: scanner.max_hops), empezando y terminando en un token base (SOL/USDC/USDT).",
  "opportunities.methodology.profit":
    "Una ruta solo cuenta si devuelve estrictamente más de lo que empezó (expected_profit > 0).",
  "opportunities.methodology.minProfit":
    "También tiene que superar un margen mínimo de {minProfitBps} bps (config: scanner.min_profit) — una ruta que apenas empata no cuenta como oportunidad real.",
  "opportunities.methodology.impact":
    "max_price_impact_bps es el peor impacto de precio en un solo salto de la ruta, según el modelo de precios propio de cada DEX.",
  "opportunities.methodology.fragile":
    "Una oportunidad se marca como frágil cuando ese impacto ya consume todo el margen (max_price_impact_bps ≥ profit_margin_bps) — es decir, cualquier demora o tamaño real de ejecución se espera que borre la ganancia. Las oportunidades frágiles no se descartan, solo se etiquetan.",

  "research.title": "Investigación",
  "research.totalOpportunities": "Oportunidades totales",
  "research.profitable": "Rentables",
  "research.profitableNotFragile": "Rentables y no frágiles",
  "research.avgProfit": "Ganancia promedio (lamports)",
  "research.chartCaption":
    "Margen vs. impacto de precio — un punto arriba de la diagonal significa que el impacto ya cotizado por el AMM consume todo el margen (frágil)",
  "research.axisMargin": "margen de ganancia (bps)",
  "research.axisImpact": "impacto de precio máx. (bps)",
  "research.sampleNote":
    "Mostrando {sampleSize} de {total} totales — las estadísticas de arriba se calculan sobre esta muestra, no sobre todo el historial.",

  "language.label": "Idioma",
  "theme.switchToDark": "Cambiar a modo oscuro",
  "theme.switchToLight": "Cambiar a modo claro",
};

const nl: typeof en = {
  "nav.overview": "Overzicht",
  "nav.pools": "Pools",
  "nav.opportunities": "Kansen",
  "nav.research": "Onderzoek",

  "overview.title": "Overzicht",
  "overview.apiUp": "API actief",
  "overview.apiDown": "API uit",
  "overview.wsConnected": "WS verbonden",
  "overview.wsDisconnected": "WS niet verbonden",
  "overview.opportunitiesPerTick": "Kansen/tick",
  "overview.poolUpdatesSec": "Pool-updates/sec",
  "overview.scannerMs": "Scanner ms",
  "overview.simulationMs": "Simulatie ms",
  "overview.liveEvents": "Live gebeurtenissen ({count})",
  "overview.waitingForEvents": "Wachten op gebeurtenissen via WS…",

  "pools.title": "Pools ({count})",
  "pools.filterPlaceholder": "Filter op pool-adres",
  "pools.loading": "Laden…",
  "pools.colDex": "DEX",
  "pools.colTokenA": "Token A",
  "pools.colTokenB": "Token B",
  "pools.colReady": "Klaar",
  "pools.ready": "klaar",
  "pools.notReady": "laden",
  "pools.detailTitle": "Pool-detail",
  "pools.detailId": "Pool-adres",
  "pools.clickForDetail": "Klik op een rij voor de volledige adressen",
  "pools.paginationInfo": "Pagina {page} van {totalPages}",

  "opportunities.title": "Kansen ({count})",
  "opportunities.subtitle": "Historisch, uit SQLite — laatste 500 rijen.",
  "opportunities.loading": "Laden…",
  "opportunities.empty":
    "Nog niets geregistreerd. Dat is een eerlijke lege staat, geen bug — zie \"Hoe een kans wordt gedefinieerd\" hieronder voor waarom de meeste kandidaat-routes niet in aanmerking komen.",
  "opportunities.colSlot": "Slot",
  "opportunities.colProfit": "Winst (lamports)",
  "opportunities.colMargin": "Marge (bps)",
  "opportunities.colImpact": "Max. impact (bps)",
  "opportunities.colFragile": "Fragiel",
  "opportunities.fragile": "fragiel",
  "opportunities.solid": "solide",
  "opportunities.colRoute": "Route",
  "opportunities.hops": "{count} sprongen",

  "opportunities.methodology.title": "Hoe een kans wordt gedefinieerd",
  "opportunities.methodology.notional":
    "Elke route wordt gequoteerd met een vast bedrag van 1 SOL (voorlopig hardcoded — niet per pool aangepast).",
  "opportunities.methodology.hops":
    "Routes gaan van 2 tot {maxHops} sprongen (config: scanner.max_hops), beginnend en eindigend op een basistoken (SOL/USDC/USDT).",
  "opportunities.methodology.profit":
    "Een route telt alleen mee als hij strikt meer teruggeeft dan hij begon (expected_profit > 0).",
  "opportunities.methodology.minProfit":
    "Hij moet ook een minimale winstmarge van {minProfitBps} bps halen (config: scanner.min_profit) — een route die net quitte speelt telt niet als echte kans.",
  "opportunities.methodology.impact":
    "max_price_impact_bps is de slechtste prijsimpact van één sprong in de route, volgens het eigen prijsmodel van elke DEX.",
  "opportunities.methodology.fragile":
    "Een kans wordt als fragiel gemarkeerd wanneer die impact de hele marge al opsoupt (max_price_impact_bps ≥ profit_margin_bps) — elke echte uitvoeringsvertraging of -omvang zou de winst dan tenietdoen. Fragiele kansen worden niet weggefilterd, alleen gelabeld.",

  "research.title": "Onderzoek",
  "research.totalOpportunities": "Totaal aantal kansen",
  "research.profitable": "Winstgevend",
  "research.profitableNotFragile": "Winstgevend & niet fragiel",
  "research.avgProfit": "Gem. winst (lamports)",
  "research.chartCaption":
    "Marge vs. prijsimpact — een punt boven de diagonaal betekent dat de eigen impact-schatting van de AMM de marge al opsoupt (fragiel)",
  "research.axisMargin": "winstmarge (bps)",
  "research.axisImpact": "max. prijsimpact (bps)",
  "research.sampleNote":
    "Toont {sampleSize} van {total} totaal — bovenstaande statistieken zijn berekend over deze steekproef, niet over de hele geschiedenis.",

  "language.label": "Taal",
  "theme.switchToDark": "Overschakelen naar donkere modus",
  "theme.switchToLight": "Overschakelen naar lichte modus",
};

export const TRANSLATIONS: Record<Locale, typeof en> = { en, es, nl };
export type TranslationKey = keyof typeof en;
