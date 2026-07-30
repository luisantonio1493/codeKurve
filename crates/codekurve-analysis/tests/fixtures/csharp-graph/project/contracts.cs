namespace Acme.Contracts;

public interface IInvoiceProcessor
{
    decimal Process(decimal amount);
}

public class InvoiceBase
{
    public virtual decimal Calculate(decimal amount) => amount;
}
