import { useQuery } from "@tanstack/react-query";
import { Badge, Card, Group, SimpleGrid, Stack, Text, Title } from "@mantine/core";
import { fetchHealth, fetchMetrics } from "../api";
import { useLiveStore } from "../store";
import { useT } from "../i18n/useLocale";

function Stat({ label, value }: { label: string; value: string | number }) {
  return (
    <Card withBorder padding="md">
      <Text size="xs" c="dimmed" tt="uppercase">
        {label}
      </Text>
      <Text size="xl" fw={700}>
        {value}
      </Text>
    </Card>
  );
}

export function Overview() {
  const t = useT();
  const health = useQuery({
    queryKey: ["health"],
    queryFn: fetchHealth,
    retry: false,
    refetchInterval: 5000,
  });
  const metrics = useQuery({
    queryKey: ["metrics"],
    queryFn: fetchMetrics,
    refetchInterval: 5000,
  });
  const connected = useLiveStore((s) => s.connected);
  const events = useLiveStore((s) => s.events);

  return (
    <Stack>
      <Group justify="space-between">
        <Title order={2}>{t("overview.title")}</Title>
        <Group gap="xs">
          <Badge color={health.isSuccess ? "green" : "red"}>
            {health.isSuccess ? t("overview.apiUp") : t("overview.apiDown")}
          </Badge>
          <Badge color={connected ? "green" : "gray"}>
            {connected ? t("overview.wsConnected") : t("overview.wsDisconnected")}
          </Badge>
        </Group>
      </Group>

      {metrics.data && (
        <SimpleGrid cols={{ base: 2, sm: 4 }}>
          <Stat label={t("overview.opportunitiesPerTick")} value={metrics.data.opportunities_found} />
          <Stat label={t("overview.poolUpdatesSec")} value={metrics.data.pool_updates_sec.toFixed(1)} />
          <Stat label={t("overview.scannerMs")} value={metrics.data.scanner_latency_ms.toFixed(1)} />
          <Stat label={t("overview.simulationMs")} value={metrics.data.simulation_latency_ms.toFixed(1)} />
        </SimpleGrid>
      )}

      <Card withBorder>
        <Text fw={600} mb="xs">
          {t("overview.liveEvents", { count: events.length })}
        </Text>
        <Stack gap={4} mah={300} style={{ overflowY: "auto" }}>
          {events.length === 0 && (
            <Text size="sm" c="dimmed">
              {t("overview.waitingForEvents")}
            </Text>
          )}
          {events.map((e) => (
            <Text key={e.receivedAt} size="xs" ff="monospace">
              {JSON.stringify(e.raw)}
            </Text>
          ))}
        </Stack>
      </Card>
    </Stack>
  );
}
