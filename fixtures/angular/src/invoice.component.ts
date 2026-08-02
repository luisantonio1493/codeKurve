// Routed component, standalone, with a constructor-DI'd service — the
// route -> component -> injected-service chain (task 7.1).
@Component({
  selector: 'app-invoice',
  standalone: true,
  imports: [SharedModule],
  providers: [],
})
export class InvoiceComponent {
  constructor(private repo: InvoiceApiRepository) {}
}
