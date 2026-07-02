import { createRouter, createWebHashHistory } from "vue-router";
import ProjectView from "../views/ProjectView.vue";

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", name: "project", component: ProjectView },
    {
      path: "/catalog",
      name: "catalog",
      component: () => import("../views/CatalogView.vue"),
    },
    {
      path: "/game",
      name: "game-info",
      component: () => import("../views/GameInfoView.vue"),
    },
    {
      path: "/setup",
      name: "setup",
      component: () => import("../views/SetupView.vue"),
    },
    {
      path: "/language",
      name: "language",
      component: () => import("../views/LanguageView.vue"),
    },
    {
      path: "/diagnostics",
      name: "diagnostics",
      component: () => import("../views/DiagnosticsView.vue"),
    },
    {
      path: "/mod/:id",
      name: "mod-detail",
      component: () => import("../views/ModDetailView.vue"),
      props: true,
    },
    {
      path: "/conflicts",
      name: "conflicts",
      component: () => import("../views/ConflictView.vue"),
    },
    {
      path: "/saves",
      name: "saves",
      component: () => import("../views/SavesView.vue"),
    },
    // Kept reachable by URL, but no longer in the sidebar (the "Build & Deploy"
    // tab became "Save backups").
    {
      path: "/export",
      name: "export",
      component: () => import("../views/ExportView.vue"),
    },
  ],
});
