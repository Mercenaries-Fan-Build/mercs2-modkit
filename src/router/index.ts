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
      path: "/setup",
      name: "setup",
      component: () => import("../views/SetupView.vue"),
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
      path: "/export",
      name: "export",
      component: () => import("../views/ExportView.vue"),
    },
  ],
});
