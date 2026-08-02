using Acme.Invoicing.Data;

namespace Acme.Invoicing.Api;

// Attribute-driven recognition (PR5): [ApiController] + class-level [Route]
// prefix joined with the method's own [HttpGet] template.
[ApiController]
[Route("api/invoices")]
public class InvoiceController : ControllerBase
{
    private readonly IInvoiceRepository _repository;

    public InvoiceController(IInvoiceRepository repository)
    {
        _repository = repository;
    }

    [HttpGet("{id}")]
    public Invoice GetById(int id) => _repository.Find(id);
}
