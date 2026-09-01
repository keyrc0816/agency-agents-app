import type { Locale } from "$lib/i18n/messages";
import type { Agent, AgentLocalization } from "$lib/types";

export function agentLocalization(
  agent: Agent,
  locale: Locale | string,
): AgentLocalization | null {
  return agent.localizations?.[locale] ?? null;
}

export function agentDisplayName(agent: Agent, locale: Locale | string): string {
  const localized = locale === "zh-TW" ? agentLocalization(agent, locale) : null;
  return localized ? `${agent.name}｜${localized.name}` : agent.name;
}

export function agentLocalizedDescription(
  agent: Agent,
  locale: Locale | string,
): string | null {
  return locale === "zh-TW"
    ? (agentLocalization(agent, locale)?.description ?? null)
    : null;
}

export function normalizeAgentSearch(value: string, locale: Locale | string): string {
  return value.normalize("NFKC").toLocaleLowerCase(locale);
}

export function agentSearchText(agent: Agent, locale: Locale | string): string {
  const localized = agentLocalization(agent, "zh-TW");
  return normalizeAgentSearch(
    [
      agent.name,
      agent.description,
      agent.vibe ?? "",
      localized?.name ?? "",
      localized?.description ?? "",
    ].join(" "),
    locale,
  );
}
