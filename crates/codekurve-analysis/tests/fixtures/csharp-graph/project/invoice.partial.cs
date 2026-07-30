namespace Acme.Billing;

public partial class InvoiceProcessor
{
    public string Name { get; set; } = "invoice";
}

public class Repository<T> where T : IInvoiceProcessor
{
    public T Save(T value) => value;
}

public class ExternalInvoice : System.Object {}
