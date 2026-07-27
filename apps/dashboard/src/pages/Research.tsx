import { useQuery } from "@tanstack/react-query";
import { Card, SimpleGrid, Text, Title } from "@mantine/core";
import ReactECharts from "echarts-for-react";
import { useMemo } from "react";
import { fetchOpportunities } from "../api";
import { useT } from "../i18n/useLocale";

// "Does this strategy have a real edge?" — the question this whole phase
// exists to answer (IMPLEMENTATION_PLAN.md Phase 4/5). `/opportunities` is
// server-paginated now (200 max per request) — aggregates below are computed
// over the most recent 200 rows, not the full historical set. `total` (the
// real server-side count) is still shown as-is so it never looks like more
// data was seen than actually was.
const SAMPLE_SIZE = 200;

export function Research() {
  const t = useT();
  const { data } = useQuery({
    queryKey: ["opportunities", "research-sample"],
    queryFn: () => fetchOpportunities({ limit: SAMPLE_SIZE, offset: 0 }),
  });

  const stats = useMemo(() => {
    const rows = data?.items ?? [];
    const profitable = rows.filter((r) => r.expected_profit > 0);
    const solid = profitable.filter((r) => !r.fragile);
    const avgProfit = rows.length ? rows.reduce((s, r) => s + r.expected_profit, 0) / rows.length : 0;
    return { profitable: profitable.length, solid: solid.length, avgProfit, sampleSize: rows.length };
  }, [data]);

  const chartOption = useMemo(
    () => ({
      xAxis: { name: t("research.axisMargin"), type: "value" },
      yAxis: { name: t("research.axisImpact"), type: "value" },
      tooltip: { trigger: "item" },
      series: [
        {
          type: "scatter",
          symbolSize: 6,
          data: (data?.items ?? []).map((r) => [r.profit_margin_bps, r.max_price_impact_bps]),
        },
      ],
    }),
    [data, t],
  );

  return (
    <>
      <Title order={2} mb="md">
        {t("research.title")}
      </Title>
      <SimpleGrid cols={{ base: 2, sm: 4 }} mb="lg">
        <Card withBorder>
          <Text size="xs" c="dimmed">{t("research.totalOpportunities")}</Text>
          <Text size="xl" fw={700}>{data?.total ?? 0}</Text>
        </Card>
        <Card withBorder>
          <Text size="xs" c="dimmed">{t("research.profitable")}</Text>
          <Text size="xl" fw={700}>{stats.profitable}</Text>
        </Card>
        <Card withBorder>
          <Text size="xs" c="dimmed">{t("research.profitableNotFragile")}</Text>
          <Text size="xl" fw={700}>{stats.solid}</Text>
        </Card>
        <Card withBorder>
          <Text size="xs" c="dimmed">{t("research.avgProfit")}</Text>
          <Text size="xl" fw={700}>{stats.avgProfit.toFixed(0)}</Text>
        </Card>
      </SimpleGrid>
      {(data?.total ?? 0) > stats.sampleSize && (
        <Text size="xs" c="dimmed" mb="md">
          {t("research.sampleNote", { sampleSize: stats.sampleSize, total: data?.total ?? 0 })}
        </Text>
      )}
      <Card withBorder>
        <Text fw={600} mb="xs">
          {t("research.chartCaption")}
        </Text>
        <ReactECharts option={chartOption} style={{ height: 400 }} />
      </Card>
    </>
  );
}
