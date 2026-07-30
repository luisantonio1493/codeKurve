using Acme.Contracts;
namespace Acme.App;
public class Program
{
    public string Run() { IGreeter greeter = new Greeter(); return greeter.Greet("world"); }
}
