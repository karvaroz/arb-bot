import {
  ActionIcon,
  AppShell,
  Burger,
  Group,
  NavLink,
  Select,
  Title,
  useComputedColorScheme,
  useMantineColorScheme,
} from "@mantine/core";
import { useDisclosure } from "@mantine/hooks";
import { IconChartDots, IconDroplet, IconGauge, IconMoon, IconSun, IconTarget } from "@tabler/icons-react";
import { NavLink as RouterNavLink, Route, Routes, useLocation } from "react-router-dom";
import { useEffect } from "react";
import { Overview } from "./pages/Overview";
import { Pools } from "./pages/Pools";
import { Opportunities } from "./pages/Opportunities";
import { Research } from "./pages/Research";
import { useLiveEvents } from "./useLiveEvents";
import { useT, useLocaleStore } from "./i18n/useLocale";
import { LOCALES, LOCALE_LABELS, type Locale } from "./i18n/translations";

function ThemeToggle() {
  const t = useT();
  const { setColorScheme } = useMantineColorScheme();
  const computedScheme = useComputedColorScheme("light");
  const isDark = computedScheme === "dark";

  return (
    <ActionIcon
      variant="subtle"
      size="lg"
      aria-label={isDark ? t("theme.switchToLight") : t("theme.switchToDark")}
      onClick={() => setColorScheme(isDark ? "light" : "dark")}
    >
      {isDark ? <IconSun size={18} /> : <IconMoon size={18} />}
    </ActionIcon>
  );
}

export default function App() {
  useLiveEvents();
  const t = useT();
  const locale = useLocaleStore((s) => s.locale);
  const setLocale = useLocaleStore((s) => s.setLocale);
  const [mobileOpened, { toggle: toggleMobile, close: closeMobile }] = useDisclosure(false);
  const location = useLocation();

  // Close the mobile drawer whenever the route changes — otherwise picking
  // a page leaves the overlay open on top of it.
  useEffect(() => {
    closeMobile();
  }, [location.pathname, closeMobile]);

  const nav = [
    { to: "/", label: t("nav.overview"), icon: IconGauge },
    { to: "/pools", label: t("nav.pools"), icon: IconDroplet },
    { to: "/opportunities", label: t("nav.opportunities"), icon: IconTarget },
    { to: "/research", label: t("nav.research"), icon: IconChartDots },
  ];

  return (
    <AppShell
      header={{ height: 56 }}
      navbar={{ width: 220, breakpoint: "sm", collapsed: { mobile: !mobileOpened } }}
      padding="md"
    >
      <AppShell.Header hiddenFrom="sm">
        <Group h="100%" px="md" justify="space-between">
          <Group>
            <Burger opened={mobileOpened} onClick={toggleMobile} size="sm" />
            <Title order={4}>arb-bot</Title>
          </Group>
          <ThemeToggle />
        </Group>
      </AppShell.Header>
      <AppShell.Navbar p="md">
        <Group mb="lg" visibleFrom="sm" justify="space-between">
          <Title order={4}>arb-bot</Title>
          <ThemeToggle />
        </Group>
        {nav.map(({ to, label, icon: Icon }) => (
          <NavLink
            key={to}
            component={RouterNavLink}
            to={to}
            label={label}
            leftSection={<Icon size={18} />}
            end={to === "/"}
          />
        ))}
        <Select
          mt="lg"
          label={t("language.label")}
          value={locale}
          onChange={(value) => value && setLocale(value as Locale)}
          data={LOCALES.map((l) => ({ value: l, label: LOCALE_LABELS[l] }))}
          allowDeselect={false}
        />
      </AppShell.Navbar>
      <AppShell.Main>
        <Routes>
          <Route path="/" element={<Overview />} />
          <Route path="/pools" element={<Pools />} />
          <Route path="/opportunities" element={<Opportunities />} />
          <Route path="/research" element={<Research />} />
        </Routes>
      </AppShell.Main>
    </AppShell>
  );
}
