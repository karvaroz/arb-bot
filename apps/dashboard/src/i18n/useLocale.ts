import { create } from "zustand";
import { persist } from "zustand/middleware";
import { LOCALES, TRANSLATIONS, type Locale, type TranslationKey } from "./translations";

function detectDefaultLocale(): Locale {
  const nav = navigator.language.slice(0, 2);
  return (LOCALES as readonly string[]).includes(nav) ? (nav as Locale) : "en";
}

interface LocaleState {
  locale: Locale;
  setLocale: (locale: Locale) => void;
}

export const useLocaleStore = create<LocaleState>()(
  persist(
    (set) => ({
      locale: detectDefaultLocale(),
      setLocale: (locale) => set({ locale }),
    }),
    { name: "arb-bot-dashboard-locale" },
  ),
);

/** `t("overview.liveEvents", { count: 3 })` -> "Live events (3)" */
export function useT() {
  const locale = useLocaleStore((s) => s.locale);
  return (key: TranslationKey, vars?: Record<string, string | number>) => {
    let str = TRANSLATIONS[locale][key] ?? TRANSLATIONS.en[key];
    if (vars) {
      for (const [k, v] of Object.entries(vars)) {
        str = str.replace(`{${k}}`, String(v));
      }
    }
    return str;
  };
}
