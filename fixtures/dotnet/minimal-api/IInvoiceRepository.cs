namespace Acme.Invoicing.MinimalApi.Data;

public interface IInvoiceRepository
{
    Invoice Find(int id);
}
