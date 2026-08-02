namespace Acme.Invoicing.Data;

public class InvoiceRepository : IInvoiceRepository
{
    private readonly AppDbContext _context;

    public InvoiceRepository(AppDbContext context)
    {
        _context = context;
    }

    public Invoice Find(int id) => _context.Invoices.Find(id);
}
