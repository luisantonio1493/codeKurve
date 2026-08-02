namespace Acme.Invoicing.Data;

public interface IInvoiceRepository
{
    Invoice Find(int id);
}
