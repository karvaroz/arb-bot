import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { createColumnHelper, flexRender, getCoreRowModel, useReactTable } from "@tanstack/react-table";
import { ActionIcon, Badge, Group, Modal, Pagination, Stack, Table, Text, TextInput, Title } from "@mantine/core";
import { useDisclosure } from "@mantine/hooks";
import { IconCopy } from "@tabler/icons-react";
import { useEffect, useMemo, useState } from "react";
import { fetchPools, type Pool } from "../api";
import { useT } from "../i18n/useLocale";

const columnHelper = createColumnHelper<Pool>();
const PAGE_SIZE = 20;

function AddressRow({ label, value }: { label: string; value: string }) {
  return (
    <Stack gap={2}>
      <Text size="xs" c="dimmed" tt="uppercase">
        {label}
      </Text>
      <Group gap="xs" wrap="nowrap">
        <Text ff="monospace" size="sm" style={{ wordBreak: "break-all" }}>
          {value}
        </Text>
        <ActionIcon variant="subtle" size="sm" onClick={() => navigator.clipboard.writeText(value)}>
          <IconCopy size={14} />
        </ActionIcon>
      </Group>
    </Stack>
  );
}

export function Pools() {
  const t = useT();
  const [filterInput, setFilterInput] = useState("");
  const [filter, setFilter] = useState("");
  const [pageIndex, setPageIndex] = useState(0);
  const [selected, setSelected] = useState<Pool | null>(null);
  const [modalOpened, { open: openModal, close: closeModal }] = useDisclosure(false);

  // Debounce the search box so every keystroke doesn't fire a new request —
  // the filter itself runs server-side now (`q`), not against an
  // already-fetched full list.
  useEffect(() => {
    const id = setTimeout(() => {
      setFilter(filterInput);
      setPageIndex(0);
    }, 300);
    return () => clearTimeout(id);
  }, [filterInput]);

  const { data, isLoading, isPlaceholderData } = useQuery({
    queryKey: ["pools", pageIndex, filter],
    queryFn: () => fetchPools({ limit: PAGE_SIZE, offset: pageIndex * PAGE_SIZE, q: filter || undefined }),
    refetchInterval: 10000,
    placeholderData: keepPreviousData,
  });

  const columns = useMemo(
    () => [
      columnHelper.accessor("dex", { header: t("pools.colDex") }),
      columnHelper.accessor("token_a", {
        header: t("pools.colTokenA"),
        cell: (c) => (
          <Text ff="monospace" size="xs">
            {c.getValue().slice(0, 8)}…
          </Text>
        ),
      }),
      columnHelper.accessor("token_b", {
        header: t("pools.colTokenB"),
        cell: (c) => (
          <Text ff="monospace" size="xs">
            {c.getValue().slice(0, 8)}…
          </Text>
        ),
      }),
      columnHelper.accessor("is_ready", {
        header: t("pools.colReady"),
        cell: (c) => (
          <Badge color={c.getValue() ? "green" : "yellow"}>
            {c.getValue() ? t("pools.ready") : t("pools.notReady")}
          </Badge>
        ),
      }),
    ],
    [t],
  );

  const table = useReactTable({ data: data?.items ?? [], columns, getCoreRowModel: getCoreRowModel() });
  const totalPages = Math.max(1, Math.ceil((data?.total ?? 0) / PAGE_SIZE));

  return (
    <>
      <Title order={2} mb="md">
        {t("pools.title", { count: data?.total ?? 0 })}
      </Title>
      <TextInput
        placeholder={t("pools.filterPlaceholder")}
        value={filterInput}
        onChange={(e) => setFilterInput(e.currentTarget.value)}
        mb="xs"
      />
      <Text size="xs" c="dimmed" mb="sm">
        {t("pools.clickForDetail")}
      </Text>
      {isLoading ? (
        <Text>{t("pools.loading")}</Text>
      ) : (
        <>
          <Table.ScrollContainer minWidth={500}>
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
                  <Table.Tr
                    key={row.id}
                    style={{ cursor: "pointer" }}
                    onClick={() => {
                      setSelected(row.original);
                      openModal();
                    }}
                  >
                    {row.getVisibleCells().map((cell) => (
                      <Table.Td key={cell.id}>{flexRender(cell.column.columnDef.cell, cell.getContext())}</Table.Td>
                    ))}
                  </Table.Tr>
                ))}
              </Table.Tbody>
            </Table>
          </Table.ScrollContainer>
          <Group justify="space-between" mt="sm">
            <Text size="sm" c="dimmed">
              {t("pools.paginationInfo", { page: pageIndex + 1, totalPages })}
            </Text>
            <Pagination total={totalPages} value={pageIndex + 1} onChange={(page) => setPageIndex(page - 1)} />
          </Group>
        </>
      )}

      <Modal opened={modalOpened} onClose={closeModal} title={t("pools.detailTitle")}>
        {selected && (
          <Stack>
            <AddressRow label={t("pools.detailId")} value={selected.id} />
            <AddressRow label={t("pools.colTokenA")} value={selected.token_a} />
            <AddressRow label={t("pools.colTokenB")} value={selected.token_b} />
            <Group>
              <Text size="sm" fw={600}>
                {t("pools.colDex")}:
              </Text>
              <Text size="sm">{selected.dex}</Text>
            </Group>
            <Group>
              <Text size="sm" fw={600}>
                {t("pools.colReady")}:
              </Text>
              <Badge color={selected.is_ready ? "green" : "yellow"}>
                {selected.is_ready ? t("pools.ready") : t("pools.notReady")}
              </Badge>
            </Group>
          </Stack>
        )}
      </Modal>
    </>
  );
}
