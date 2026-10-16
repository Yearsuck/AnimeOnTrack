import { useState } from "react";
import { useT } from "../i18n";
import type { SiteSummary } from "../types";
import "../styles/settings.css";
import { AppearanceSection } from "./settings/AppearanceSection";
import { PlayerSettingsCard } from "./settings/PlayerSettingsCard";
import { SourcesSection } from "./settings/SourcesSection";
import { LibraryImportCard } from "./settings/LibraryImportCard";
import { BackupCard } from "./settings/BackupCard";
import { StorageCard } from "./settings/StorageCard";
import { MaintenanceSection } from "./settings/MaintenanceSection";
import { UninstallCard } from "./settings/UninstallCard";

type SettingsTab = "general" | "sources" | "data" | "maintenance";

const TAB_STORAGE_KEY = "aot.settings.tab";

function getInitialTab(): SettingsTab {
  try {
    const saved = localStorage.getItem(TAB_STORAGE_KEY);
    if (saved === "general" || saved === "sources" || saved === "data" || saved === "maintenance") {
      return saved;
    }
  } catch {
    // localStorage unavailable, fallback to default
  }
  return "general";
}

function saveTab(tab: SettingsTab) {
  try {
    localStorage.setItem(TAB_STORAGE_KEY, tab);
  } catch {
    // localStorage unavailable, ignore
  }
}

export function Settings({ onSiteChanged }: { onSiteChanged?: (site: SiteSummary) => void }) {
  const t = useT();
  const [activeTab, setActiveTab] = useState<SettingsTab>(getInitialTab);
  // A panel mounts on its first visit (so tabs never opened fire no IPC) and
  // then stays mounted, hidden: unsaved mirrors/credentials and a running
  // library import must survive switching tabs.
  const [visited, setVisited] = useState<ReadonlySet<SettingsTab>>(() => new Set([activeTab]));

  const selectTab = (tab: SettingsTab) => {
    setActiveTab(tab);
    setVisited((prev) => (prev.has(tab) ? prev : new Set(prev).add(tab)));
    saveTab(tab);
  };

  return (
    <div className="page settings-page settings-page-wide">
      <div className="page-head">
        <h2 className="page-title">{t("nav.settings")}</h2>
      </div>

      <div className="settings-layout">
        <nav className="settings-nav" role="tablist" aria-label={t("settings.tablistAria")}>
          <button
            id="tab-general"
            role="tab"
            type="button"
            className={`settings-nav-item ${activeTab === "general" ? "active" : ""}`}
            aria-selected={activeTab === "general"}
            aria-controls="panel-general"
            onClick={() => selectTab("general")}
          >
            {t("settings.tabGeneral")}
          </button>
          <button
            id="tab-sources"
            role="tab"
            type="button"
            className={`settings-nav-item ${activeTab === "sources" ? "active" : ""}`}
            aria-selected={activeTab === "sources"}
            aria-controls="panel-sources"
            onClick={() => selectTab("sources")}
          >
            {t("settings.tabSources")}
          </button>
          <button
            id="tab-data"
            role="tab"
            type="button"
            className={`settings-nav-item ${activeTab === "data" ? "active" : ""}`}
            aria-selected={activeTab === "data"}
            aria-controls="panel-data"
            onClick={() => selectTab("data")}
          >
            {t("settings.tabData")}
          </button>
          <button
            id="tab-maintenance"
            role="tab"
            type="button"
            className={`settings-nav-item ${activeTab === "maintenance" ? "active" : ""}`}
            aria-selected={activeTab === "maintenance"}
            aria-controls="panel-maintenance"
            onClick={() => selectTab("maintenance")}
          >
            {t("settings.tabMaintenance")}
          </button>
        </nav>

        <div className="settings-panels">
          <section
            hidden={activeTab !== "general"}
            id="panel-general"
            role="tabpanel"
            aria-labelledby="tab-general"
            className="settings-panel"
          >
            {visited.has("general") && (
              <>
                <AppearanceSection />
                <PlayerSettingsCard />
              </>
            )}
          </section>

          <section
            hidden={activeTab !== "sources"}
            id="panel-sources"
            role="tabpanel"
            aria-labelledby="tab-sources"
            className="settings-panel"
          >
            {visited.has("sources") && (
              <>
                <SourcesSection onSiteChanged={onSiteChanged} />
              </>
            )}
          </section>

          <section
            hidden={activeTab !== "data"}
            id="panel-data"
            role="tabpanel"
            aria-labelledby="tab-data"
            className="settings-panel"
          >
            {visited.has("data") && (
              <>
                <LibraryImportCard />
                <BackupCard />
                <StorageCard />
              </>
            )}
          </section>

          <section
            hidden={activeTab !== "maintenance"}
            id="panel-maintenance"
            role="tabpanel"
            aria-labelledby="tab-maintenance"
            className="settings-panel"
          >
            {visited.has("maintenance") && (
              <>
                <MaintenanceSection />
                <UninstallCard />
              </>
            )}
          </section>
        </div>
      </div>
    </div>
  );
}
