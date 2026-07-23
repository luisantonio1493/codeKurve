import { Base } from './base';
import { helper } from './utils';
import { Missing } from './nonexistent';

class Foo extends Base implements IFoo {
  run(): number {
    helper();
    return this.helper2();
  }

  helper2(): number {
    return 2;
  }
}

export { Foo };

function build(): Base {
  return new Base();
}
