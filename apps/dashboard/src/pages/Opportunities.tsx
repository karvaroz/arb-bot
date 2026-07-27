import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { createColumnHelper, flexRender, getCoreRowModel, useReactTable } from "@tanstack/react-table";
import { Badge, Card, Group, List, Pagination, Table, Text, Title } from "@mantine/core";
import { useMemo, useState } from "react";
import { fetchConfig, fetchOpportunities, type OpportunityRow } from "../api";
import { useT } from "../i18n/useLocale";

const columnHelper = createColumnHelper<OpportunityRow>();
const PAGE_SIZE = 50;

function Methodology() {
  const t = useT();
  const { data: config } = useQuery({ queryKey: ["config"], queryFn: fetchConfig });

  return (
    <Card withBorder mb="md">
      <Text fw={600} mb="xs">
        {t("opportunities.methodology.title")}
      </Text>
      <List size="sm" spacing={4}>
        <List.Item>{t("opportunities.methodology.notional")}</List.Item>
        <List.Item>
          {t("opportunities.methodology.hops", { maxHops: config?.max_hops ?? "…" })}
        </List.Item>
        <List.Item>{t("opportunities.methodology.profit")}</List.Item>
        <List.Item>
          {t("opportunities.methodology.minProfit", { minProfitBps: config?.min_profit_bps ?? "…" })}
        </List.Item>
        <List.Item>{t("opportunities.methodology.impact")}</List.Item>
        <List.Item>{t("opportunities.methodology.fragile")}</List.Item>
      </List>
    </Card>
  );
}

export function Opportunities() {
  const t = useT();
  const [pageIndex, setPageIndex] = useState(0);

  const { data, isLoading, isPlaceholderData } = useQuery({
    queryKey: ["opportunities", pageIndex],
    queryFn: () => fetchOpportunities({ limit: PAGE_SIZE, offset: pageIndex * PAGE_SIZE }),
    refetchInterval: 10000,
    placeholderData: keepPreviousData,
  });

  const columns = useMemo(
    () => [
      columnHelper.accessor("slot", { header: t("opportunities.colSlot") }),
      columnHelper.accessor("expected_profit", { header: t("opportunities.colProfit") }),
      columnHelper.accessor("profit_margin_bps", { header: t("opportunities.colMargin") }),
      columnHelper.accessor("max_price_impact_bps", { header: t("opportunities.colImpact") }),
      columnHelper.accessor("fragile", {
        header: t("opportunities.colFragile"),
        cell: (c) => (
          <Badge color={c.getValue() ? "red" : "green"}>
            {c.getValue() ? t("opportunities.fragile") : t("opportunities.solid")}
          </Badge>
        ),
      }),
      columnHelper.accessor("route_pools", {
        header: t("opportunities.colRoute"),
        cell: (c) => <Text size="xs">{t("opportunities.hops", { count: c.getValue().length })}</Text>,
      }),
    ],
    [t],
  );

  const table = useReactTable({ data: data?.items ?? [], columns, getCoreRowModel: getCoreRowModel() });
  const totalPages = Math.max(1, Math.ceil((data?.total ?? 0) / PAGE_SIZE));

  return (
    <>
      <Title order={2} mb="md">
        {t("opportunities.title", { count: data?.total ?? 0 })}
      </Title>
      <Text size="sm" c="dimmed" mb="sm">
        {t("opportunities.subtitle")}
      </Text>
      <Methodology />
      {isLoading ? (
        <Text>{t("opportunities.loading")}</Text>
      ) : data?.total === 0 ? (
        <Text c="dimmed">{t("opportunities.empty")}</Text>
      ) : (
        <>
          <Table.ScrollContainer minWidth={600}>
            <Table striped highlightOnHover opacity={isPlaceholderData ? 0.6 : 1}>
              <Table.Thead>
                {table.getHeaderGroups().map((hg) => (
                  <Table.Tr key={hg.id}>
                    {hg.headers.map((h) => (
                      <Table.Th key={h.id}>{flexRender(h.column.columnDef.header, h.getContext())}</Table.Th>
                    ))}
                  </Table.Tr>
                ))}
              </Table.Thead>
              <Table.Tbody>
                {table.getRowModel().rows.map((row) => (
                  <Table.Tr key={row.id}>
                    {row.getVisibleCells().map((cell) => (
                      <Table.Td key={cell.id}>{flexRender(cell.column.columnDef.cell, cell.getContext())}</Table.Td>
                    ))}
                  </Table.Tr>
                ))}
              </Table.Tbody>
            </Table>
          </Table.ScrollContainer>
          <Group justify="flex-end" mt="sm">
            <Pagination total={totalPages} value={pageIndex + 1} onChange={(page) => setPageIndex(page - 1)} />
          </Group>
        </>
      )}
    </>
  );
}
