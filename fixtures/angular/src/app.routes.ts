// Route config: canActivate guard + nested children + a lazy-loaded route
// (task 4.8/4.9) — the route -> component chain the E2E test traces.
const routes: Routes = [
  {
    path: 'invoices',
    canActivate: [AuthGuard],
    children: [
      { path: '', component: InvoiceListComponent },
      { path: ':id', component: InvoiceComponent },
      {
        path: ':id/lazy',
        loadComponent: () => import('./invoice-lazy.component').then(m => m.InvoiceLazyComponent),
      },
    ],
  },
];
