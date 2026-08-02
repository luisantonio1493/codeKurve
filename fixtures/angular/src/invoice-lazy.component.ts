// Lazily-loaded standalone component, referenced only via `loadComponent`
// in app.routes.ts (task 4.9) — Medium-confidence `HandlesRoute` edge.
@Component({
  selector: 'app-invoice-lazy',
  standalone: true,
})
export class InvoiceLazyComponent {
  constructor(private repo: InvoiceApiRepository) {}
}
