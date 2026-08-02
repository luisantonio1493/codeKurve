// A second routed component, DI'd via the `inject()` function form rather
// than a constructor parameter (task 4.4).
@Component({
  selector: 'app-invoice-list',
  standalone: true,
})
export class InvoiceListComponent {
  private repo = inject(InvoiceApiRepository);
}
